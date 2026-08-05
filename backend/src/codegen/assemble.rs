//! Assembling a target's emission groups and resolving what they reference.
//!
//! The stage between the per-language emitters and rendering: gather every group
//! a model produces for one target, record which group ended up declaring each
//! symbol, and re-point the references at it. That last step is what keeps import
//! collection automatic once a module spans several files, since a Symbol table
//! can only say which IR module a type belongs to.

use std::collections::BTreeSet;

use crate::codegen::casing::CasingConfig;
use crate::codegen::group::{self, Group, SymbolIndex};
use crate::codegen::layout::repoint_to_groups;
use crate::codegen::pipeline::TargetKind;
use crate::codegen::targets::{go, rust, typescript};
use crate::codegen::tree::{Decl, ModuleFile};
use crate::codegen::visibility::Exposed;
use crate::ir::{Model, Module};

/// Emit every emission group a module produces for a target: its public types,
/// one group per entry declaration, its internal group, and the test files its
/// declared tests generate.
pub(crate) fn emit_module_files(
    module: &Module,
    target: TargetKind,
    casing: &CasingConfig,
    union_ids: &std::collections::HashSet<String>,
    exposed: &Exposed,
) -> Vec<ModuleFile> {
    match target {
        TargetKind::Rust => rust::emit::emit_module(module, casing, exposed),
        TargetKind::Go => go::emit::emit_module(module, casing, union_ids, exposed),
        TargetKind::TypeScript => typescript::emit::emit_module(module, casing, exposed),
    }
}

/// The SDK-root group's file, when the target has anything to put in it: the
/// runtime helpers that serve every module, so the SDK carries one copy rather
/// than one per module. Marked internal, so each target fences it off from a
/// consumer.
pub(crate) fn shared_files(
    model: &Model,
    target: TargetKind,
    casing: &CasingConfig,
) -> Vec<ModuleFile> {
    let groups = match target {
        TargetKind::Rust => rust::emit::shared_groups(model, casing),
        TargetKind::Go => go::emit::shared_groups(model),
        TargetKind::TypeScript => typescript::emit::shared_groups(),
    };
    groups
        .into_iter()
        .filter(|(_, decls)| !decls.is_empty())
        .map(|(name, decls)| ModuleFile::new(Group::root(name), decls))
        .collect()
}

/// The SDK-root support group's declarations: the branded well-known types,
/// which belong to the SDK rather than to any one module. Only the ones
/// something references are emitted, so an SDK that never names a date does not
/// ship a date type.
pub(crate) fn support_decls(target: TargetKind, needed: &BTreeSet<String>) -> Vec<Decl> {
    keep_needed(all_support_decls(target), needed)
}

fn all_support_decls(target: TargetKind) -> Vec<Decl> {
    match target {
        TargetKind::Rust => rust::codecs::well_known_decls(),
        TargetKind::Go => go::emit::well_known_decls(),
        TargetKind::TypeScript => typescript::emit::well_known_decls(),
    }
}

/// Where the support declarations land.
///
/// They exist to be one set of types for the whole SDK, so a single-module SDK
/// has nothing to share and they ride that module's public group instead. The
/// symbol tables name the support group either way; a reference in opaque text
/// is a slot, so the spelling follows wherever the group landed rather than
/// being fixed where the text is built. What ends up unreferenced is dropped
/// once the groups are resolved.
pub(crate) fn support_file(model: &Model, target: TargetKind) -> Option<ModuleFile> {
    (model.modules.len() > 1)
        .then(|| ModuleFile::new(Group::root_support(), all_support_decls(target)))
}

/// Cut a root group down to what the rest of the SDK reaches. These groups hold
/// the machinery every module may call, but no model calls all of it: an SDK with
/// no casing transform should not ship the casing helpers, and one that never
/// names a date should not ship a date type.
fn prune_root_group(files: &mut Vec<ModuleFile>, path: &str) {
    let needed = referenced_from(files, path);
    let Some(at) = files.iter().position(|f| f.group.path() == path) else {
        return;
    };
    let decls = std::mem::take(&mut files[at].file.decls);
    files[at].file.decls = keep_needed(decls, &needed);
    if files[at].file.decls.is_empty() {
        files.remove(at);
    }
}

/// Drop the declarations nothing reaches. A declaration the tree cannot be read
/// for a name is kept, since nothing could have referenced it by name anyway. One
/// kept declaration can reach another through its own opaque text (a casing
/// helper calls the word splitter), so the wanted set grows to a fixed point
/// before anything is dropped.
fn keep_needed(decls: Vec<Decl>, needed: &BTreeSet<String>) -> Vec<Decl> {
    let mut wanted = needed.clone();
    loop {
        let reached: BTreeSet<String> = decls
            .iter()
            .filter(|decl| is_wanted(decl, &wanted))
            .filter_map(|decl| decl.opaque_text())
            .flat_map(|text| {
                names_of(&decls)
                    .into_iter()
                    .filter(move |name| mentions(text, name))
            })
            .collect();
        let before = wanted.len();
        wanted.extend(reached);
        if wanted.len() == before {
            break;
        }
    }
    decls
        .into_iter()
        .filter(|decl| is_wanted(decl, &wanted))
        .collect()
}

/// Every name the declarations declare.
fn names_of(decls: &[Decl]) -> Vec<String> {
    crate::codegen::imports::declared_symbols(decls)
}

/// Whether a declaration survives pruning: either something wants a name it
/// declares, or it declares no name to want.
fn is_wanted(decl: &Decl, wanted: &BTreeSet<String>) -> bool {
    let names = names_of(std::slice::from_ref(decl));
    names.is_empty() || names.iter().any(|name| wanted.contains(name))
}

/// Whether `text` names `name` as an identifier rather than as part of a longer
/// one, so `strSnake` is not read as a use of `str`.
fn mentions(text: &str, name: &str) -> bool {
    let boundary = |c: Option<char>| !c.is_some_and(|c| c.is_alphanumeric() || c == '_');
    text.match_indices(name).any(|(at, _)| {
        boundary(text[..at].chars().next_back()) && boundary(text[at + name.len()..].chars().next())
    })
}

/// The names a set of emitted groups references from the SDK-root group at
/// `path`. This is what makes the root groups carry exactly what is used: the
/// references are already collected from the tree, so nothing has to be guessed
/// or gated on a separate pass over the model.
fn referenced_from(files: &[ModuleFile], path: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|file| file.group.path() != path)
        .flat_map(|file| crate::codegen::imports::collect(&file.file))
        .filter(|import| import.module == path)
        .map(|import| import.imported)
        .collect()
}

/// Record which group declares each symbol, so a reference from another group
/// resolves to the file that actually holds it rather than to the module as a
/// whole. A symbol declared through opaque text is listed by the emitter
/// (`provides`), since the tree cannot be read for it.
pub(crate) fn build_index(files: &[ModuleFile]) -> SymbolIndex {
    let mut index = SymbolIndex::default();
    for file in files {
        if let Some(module) = &file.group.module {
            index.set_default(module, &Group::types(module).path());
        }
    }
    // With no root support group its declarations were folded into the single
    // module's public group, so the name the symbol tables use resolves there.
    if !files.iter().any(|f| f.group == Group::root_support()) {
        if let Some(module) = files.iter().find_map(|f| f.group.module.as_deref()) {
            index.set_default(group::ROOT_SUPPORT, &Group::types(module).path());
        }
    }
    for file in files {
        // Emitters build symbols against the IR module a type belongs to; a root
        // group's own declarations are keyed on its path, which is what the
        // emitters and the symbol tables name when they reference it.
        let owner = file
            .group
            .module
            .clone()
            .unwrap_or_else(|| file.group.path());
        let path = file.group.path();
        for name in crate::codegen::imports::declared_symbols(&file.file.decls) {
            index.insert(&owner, &name, &path);
        }
        for name in &file.provides {
            index.insert(&owner, name, &path);
        }
    }
    index
}

/// Resolve a set of emitted groups the way [`generate`] does: record which group
/// declares each symbol, then re-point every reference at it.
///
/// Exposed for a caller that drives one target's emitter directly instead of the
/// whole pipeline, which would otherwise render references still pointing at bare
/// IR module names.
pub fn resolve_groups(files: &mut Vec<ModuleFile>, target: TargetKind) {
    // Every other root group is pruned first, to what the rest of the SDK
    // reaches; the paths are read from the files themselves, since the
    // emitters own that vocabulary. Support is judged last (below): a root
    // group's own text is what may still reach a support type, so support
    // can only be pruned to what survives once nothing upstream of it can
    // shrink any further (a root group referencing a support type only in
    // declarations nothing else calls must not keep that type alive).
    // One ordered pass is sufficient, not just convenient: `Group::root_support()`
    // is the SDK's leaf group by construction (its own declarations reference
    // only other support types and language primitives — a Target's
    // `http_support_decls()`-style functions never reach back into a root
    // group), so pruning it can never re-open what an already-pruned root
    // group needed. No cycle is possible, so no fixed-point loop is needed
    // here.
    let other_roots: Vec<String> = files
        .iter()
        .filter(|f| f.group.module.is_none() && f.group != Group::root_support())
        .map(|f| f.group.path())
        .collect();
    for path in other_roots {
        prune_root_group(files, &path);
    }
    // With no group of their own, the support declarations ride the public group
    // of the module they serve. Folding here rather than at each caller is what
    // keeps a caller that drives one target's emitter directly (the round-trip
    // harnesses) from emitting a module whose types are not declared anywhere.
    if !files.iter().any(|f| f.group == Group::root_support()) {
        let needed = referenced_from(files, group::ROOT_SUPPORT);
        if let Some(types) = files.iter_mut().find(|f| f.group.name == group::TYPES) {
            let mut decls = support_decls(target, &needed);
            decls.append(&mut types.file.decls);
            types.file.decls = decls;
        }
    } else {
        prune_root_group(files, group::ROOT_SUPPORT);
    }
    let index = build_index(files);
    for file in files.iter_mut() {
        repoint_to_groups(&mut file.file, &index);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::codegen::tree::File;

    /// A file of one group referencing `names` from the group at `path`.
    fn referencing(group: Group, path: &str, names: &[&str]) -> ModuleFile {
        let decls = names
            .iter()
            .map(|name| {
                Decl::raw_with(
                    format!("uses {name}"),
                    vec![Symbol::imported(*name, path, *name)],
                )
            })
            .collect();
        ModuleFile {
            group,
            file: File {
                module: String::new(),
                decls,
            },
            provides: Vec::new(),
        }
    }

    fn named(name: &str, text: &str) -> Decl {
        Decl::raw_providing(name, text, Vec::new())
    }

    #[test]
    fn a_root_group_carries_only_what_the_rest_of_the_sdk_reaches() {
        let mut files = vec![
            ModuleFile::new(
                Group::root("duration"),
                vec![
                    named("used", "fn used() { helper() }"),
                    named("helper", "fn helper() {}"),
                    named("unused", "fn unused() {}"),
                ],
            ),
            referencing(
                Group::types("notes"),
                &Group::root("duration").path(),
                &["used"],
            ),
        ];
        prune_root_group(&mut files, &Group::root("duration").path());
        let kept: Vec<String> = crate::codegen::imports::declared_symbols(&files[0].file.decls);
        // `helper` is reached through the kept declaration's own text, which the
        // tree cannot be read for, so the wanted set has to close over it.
        assert_eq!(kept, vec!["used".to_string(), "helper".to_string()]);
    }

    #[test]
    fn a_root_group_nothing_reaches_is_not_emitted_at_all() {
        let mut files = vec![
            ModuleFile::new(Group::root_support(), vec![named("Timestamp", "type T")]),
            ModuleFile::new(Group::types("notes"), vec![named("Note", "type Note")]),
        ];
        prune_root_group(&mut files, group::ROOT_SUPPORT);
        assert!(!files.iter().any(|f| f.group == Group::root_support()));
    }

    #[test]
    fn a_declaration_the_tree_cannot_name_survives_pruning() {
        // Nothing could have referenced it by name, so dropping it would be a
        // guess; the emitters name what another group may reach.
        let decls = vec![Decl::raw("macro_rules! open_enum {}")];
        assert_eq!(keep_needed(decls.clone(), &BTreeSet::new()), decls);
    }
}

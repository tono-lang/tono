//! Assembling a target's emission groups and resolving what they reference.
//!
//! The stage between the per-language emitters and rendering: gather every group
//! a model produces for one target, record which group ended up declaring each
//! symbol, and re-point the references at it. That last step is what keeps import
//! collection automatic once a module spans several files, since a Symbol table
//! can only say which IR module a type belongs to.

use crate::codegen::casing::CasingConfig;
use crate::codegen::group::{self, Group, SymbolIndex};
use crate::codegen::layout::repoint_to_groups;
use crate::codegen::pipeline::TargetKind;
use crate::codegen::targets::{go, rust, typescript};
use crate::codegen::tree::{Decl, ModuleFile};
use crate::codegen::visibility::Exposed;
use crate::ir::{Model, Module};

/// Emit every emission group a module produces for a target: its public types,
/// one group per entry declaration, and its internal group.
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
pub(crate) fn shared_file(
    model: &Model,
    target: TargetKind,
    casing: &CasingConfig,
) -> Option<ModuleFile> {
    let decls = match target {
        TargetKind::Rust => rust::emit::shared_decls(model, casing),
        TargetKind::Go => go::emit::shared_decls(model),
        TargetKind::TypeScript => typescript::emit::shared_decls(),
    };
    (!decls.is_empty()).then(|| ModuleFile::new(Group::root_internal(), decls))
}

/// The SDK-root support group's declarations: the branded well-known types,
/// which belong to the SDK rather than to any one module.
pub(crate) fn support_decls(target: TargetKind) -> Vec<Decl> {
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
/// being fixed where the text is built.
pub(crate) fn support_file(model: &Model, target: TargetKind) -> Option<ModuleFile> {
    (model.modules.len() > 1).then(|| ModuleFile::new(Group::root_support(), support_decls(target)))
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
pub fn resolve_groups(files: &mut [ModuleFile]) {
    let index = build_index(files);
    for file in files.iter_mut() {
        repoint_to_groups(&mut file.file, &index);
    }
}

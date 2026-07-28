//! Where a target's emission groups land on disk, and how one group names
//! another.
//!
//! This is the output-layout half of module mapping; the name-mapping half (the
//! remap/flatten hooks) is in [`crate::codegen::modules`], and the group model
//! itself is in [`crate::codegen::group`]. Given already-effective (post-config)
//! module names, it decides each group's file path, its language-level import
//! path, and whether two groups share a compilation unit (in which case a
//! reference between them needs no import at all).
//!
//! Each target expresses a group with what it has. Go maps a module to a package
//! directory and each of its groups to a file in it, because a method has to live
//! in its receiver's package; only the SDK-root group, which no method touches,
//! becomes a package of its own, placed under `internal/` so nothing outside the
//! SDK can import it. Rust maps every group to a module of the crate, declared
//! private when the group is internal. TypeScript maps every group to a file, and
//! the package's `exports` map lists only the public ones.
//!
//! Every mapping here is a pure function of a group path, so the per-language
//! render rules stay stateless: they ask this module and format the statement.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::codegen::group::{self, Group, SymbolIndex};
use crate::codegen::imports::Resolver;
use crate::codegen::modules::{self, CodegenConfig};
use crate::codegen::pipeline::TargetKind;
use crate::codegen::symbol::Import;
use crate::ir::Model;

/// The name of the unit holding the SDK-root internal group: a Go package
/// directory under `internal/`, a Rust module, a TypeScript file. It has to be a
/// name no module can take (see [`check_layout`]).
pub(crate) const ROOT_UNIT: &str = "tono";

/// The directory every target fences its relocatable internal groups into. Named
/// for Go, whose toolchain refuses to resolve it from outside the SDK; the other
/// targets fence with a private module and an unlisted subpath, and share the
/// directory so one SDK has one shape.
pub(crate) const INTERNAL_DIR: &str = "internal";

/// The Go package name of the SDK-root support group. Public, so it sits beside
/// the modules rather than under `internal/`: a consumer names these types.
pub(crate) const GO_SUPPORT_PACKAGE: &str = group::SUPPORT;

/// The Go package name / Rust module leaf for a dotted module: its last segment.
pub(crate) fn package_name(module: &str) -> &str {
    module.rsplit('.').next().unwrap_or(module)
}

/// The directory a module's groups live in, relative to the target root:
/// `payments.common` -> `payments/common`.
pub(crate) fn module_dir(module: &str) -> PathBuf {
    module.split('.').collect()
}

/// Whether a Go SDK gets the shared `internal/` package. Its content is the
/// entry construction helpers, so a model with no entry declares nothing shared
/// and the package is not emitted (which is also what keeps a simple
/// single-package SDK generatable without a module path).
pub fn go_has_shared_package(model: &Model) -> bool {
    model
        .modules
        .iter()
        .any(crate::codegen::entries::has_entries)
}

/// Whether a Go SDK emits more than the modules' own packages, which is what
/// makes a cross-package import (and so a module path) unavoidable: a second
/// module, the shared packages, or any declaration Go moves under `internal/`.
pub fn go_needs_module_path(model: &Model) -> bool {
    if model.modules.len() > 1 || go_has_shared_package(model) {
        return true;
    }
    let exposed = crate::codegen::visibility::derive(model);
    model
        .modules
        .iter()
        .flat_map(|m| m.shapes.iter())
        .any(|shape| !exposed.shape(shape))
}

/// Where a group's source file lands, relative to the output root.
pub fn output_path(target: TargetKind, grp: &Group) -> PathBuf {
    let root = PathBuf::from(target.dir());
    let ext = target.extension();
    match (&grp.module, target) {
        // Each target fences an internal group with what its own ecosystem
        // reaches for, which is not the same shape in all three.
        //
        // Go has one fence and it is positional: the `internal/` directory the
        // toolchain refuses to resolve from outside the SDK. So every group Go
        // can move becomes a package under it.
        //
        // TypeScript has no per-symbol fence (an export is an export), so the
        // fence is the package's `exports` map plus what the barrel names, and
        // neither is positional: a directory buys nothing a reader can rely on.
        // So a file sits where its name says what it is, the SDK-root groups at
        // the package root and a module's internal group in the module's own
        // file, and the manifest decides what a consumer can reach.
        //
        // Rust needs no relocation at all: `mod` without `pub` fences a module
        // where it sits, so an `internal` module beside what it serves is both
        // the fence and the convention (bitflags, nom, bytemuck, crossbeam-epoch
        // all carry a private `src/internal.rs`). Moving it into a parallel tree
        // would break the rule a Rust reader relies on, that the file tree is the
        // module tree.
        // Each SDK-root group is named for what it holds, so the unit it lands in
        // takes that name: `codec` and `config` tell a reader what is inside,
        // which a name the generator picked for itself never would.
        (None, TargetKind::Go) if grp.is_internal() => root
            .join(INTERNAL_DIR)
            .join(&grp.name)
            .join(format!("{}.{ext}", grp.name)),
        // Rust and TypeScript need no directory: a private `mod` and an unlisted
        // subpath fence a file where it sits, so the name is the whole signal.
        (None, _) if grp.is_internal() => root.join(format!("{}.{ext}", grp.name)),
        (None, TargetKind::Go) => root
            .join(GO_SUPPORT_PACKAGE)
            .join(format!("{GO_SUPPORT_PACKAGE}.{ext}")),
        (None, _) => root.join(format!("{}.{ext}", grp.name)),
        // Go needs a package directory; a TypeScript module is a single file, so
        // the module path becomes the file name.
        (Some(module), TargetKind::Go) if grp.is_internal() && !grp.colocated => root
            .join(INTERNAL_DIR)
            .join(module_dir(module))
            .join(format!("{}.{ext}", package_name(module))),
        (Some(module), TargetKind::TypeScript) if grp.is_internal() && !grp.colocated => root
            .join(module_dir(module))
            .join(format!("{}.{ext}", group::TYPES)),
        // Rust needs no unit of its own for it: the declarations ride the
        // module's public file with crate visibility, which is how a Rust SDK
        // says "part of this module, not part of its surface". A module named
        // for its audience would say nothing about what it holds.
        (Some(module), TargetKind::Rust) if grp.is_internal() && !grp.colocated => root
            .join(module_dir(module))
            .join(format!("{}.{ext}", group::TYPES)),
        (Some(module), _) => root
            .join(module_dir(module))
            .join(format!("{}.{ext}", grp.name)),
    }
}

/// Where a group's file lands relative to the target's own root (the path with
/// the `<target>/` prefix dropped), which is what a barrel or a manifest inside
/// the target root refers to.
pub fn target_relative_path(target: TargetKind, grp: &Group) -> PathBuf {
    output_path(target, grp)
        .strip_prefix(target.dir())
        .map(Path::to_path_buf)
        .unwrap_or_default()
}

/// Whether two group paths render into the same Go package, so a reference
/// across them needs no import: a module's groups are files of one package, and
/// the SDK-root group is a package of its own.
pub fn same_go_package(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // A module's groups are one package, except the one Go moves under
    // `internal/`, which is a package of its own.
    match (go_package_of(a), go_package_of(b)) {
        (Some(x), Some(y)) => x == y,
        _ => false,
    }
}

/// The Go package a group path belongs to, as a `(module, relocated)` pair, or
/// `None` when the path is not a group path.
fn go_package_of(path: &str) -> Option<(&str, bool)> {
    let (module, name) = group::parse_path(path)?;
    match module {
        // Each SDK-root group is a package of its own.
        None => Some((name, false)),
        Some(module) => Some((module, name == group::INTERNAL)),
    }
}

/// The Rust crate path of a group, or `None` when the path is not a group path
/// (an external crate or a standard-library module).
pub fn rust_path(path: &str) -> Option<String> {
    let file = target_relative_path(TargetKind::Rust, &Group::from_path(path)?);
    let segments: Vec<String> = file
        .with_extension("")
        .iter()
        .map(|c| c.to_string_lossy().into_owned())
        .collect();
    Some(format!("crate::{}", segments.join("::")))
}

/// The Go import path of a group under the SDK's module path, or `None` when the
/// path is not a group path.
pub fn go_import(go_module: &str, path: &str) -> Option<String> {
    let (module, name) = group::parse_path(path)?;
    Some(match module {
        None if name != group::SUPPORT => format!("{go_module}/internal/{name}"),
        None => format!("{go_module}/{GO_SUPPORT_PACKAGE}"),
        Some(module) if name == group::INTERNAL => {
            format!("{go_module}/internal/{}", module.replace('.', "/"))
        }
        Some(module) => format!("{go_module}/{}", module.replace('.', "/")),
    })
}

/// The Go package selector a group is referenced through (its directory's last
/// segment), or `None` when the path is not a group path.
pub fn go_selector(path: &str) -> Option<String> {
    let (module, name) = group::parse_path(path)?;
    Some(match module {
        None if name != group::SUPPORT => name.to_string(),
        None => GO_SUPPORT_PACKAGE.to_string(),
        // The relocated group keeps the module's name as its package name; it is
        // only ever referenced from inside itself, so the two never collide.
        Some(module) => package_name(module).to_string(),
    })
}

/// The TypeScript import specifier that reaches group `to` from group `from`: a
/// relative path with the extension dropped, always anchored (`./` or `../`) so
/// it is never mistaken for a package name. `None` when either side is not a
/// group path.
pub fn ts_specifier(from: &str, to: &str) -> Option<String> {
    let file = |path: &str| -> Option<PathBuf> {
        Some(
            target_relative_path(TargetKind::TypeScript, &Group::from_path(path)?)
                .with_extension(""),
        )
    };
    let from = file(from)?;
    let to = file(to)?;
    let parts =
        |p: &Path| -> Vec<String> { p.iter().map(|c| c.to_string_lossy().into_owned()).collect() };
    let mut from_dir = parts(&from);
    from_dir.pop();
    let to_parts = parts(&to);
    let common = from_dir
        .iter()
        .zip(to_parts.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut segments: Vec<String> = vec!["..".to_string(); from_dir.len() - common];
    segments.extend_from_slice(&to_parts[common..]);
    let joined = segments.join("/");
    Some(if joined.starts_with("..") {
        joined
    } else {
        format!("./{joined}")
    })
}

/// Re-point every reference in a file at the group that declares it.
///
/// This is what keeps import collection automatic once a module spans several
/// files: the symbol tables still speak in IR modules, because that is all a
/// Symbol table knows, and the index built over the emitted files says which
/// group each name ended up in. A symbol the index does not know is not the
/// SDK's (a standard-library or runtime-package import) and is left alone.
pub fn repoint_to_groups(file: &mut crate::codegen::tree::File, index: &SymbolIndex) {
    crate::codegen::imports::repoint(file, &|symbol| {
        if let Some(import) = &mut symbol.import {
            if let Some(group) = index.group_of(&import.module, &import.imported) {
                import.module = group.to_string();
            }
        }
    });
}

/// Drops a reference that lands in the importing file's own compilation unit and
/// keeps every other one. Run after [`repoint_to_groups`], so both sides are
/// group paths (or, for a symbol outside the SDK, a package specifier that is in
/// nobody's unit).
pub struct SameUnit {
    pub target: TargetKind,
}

impl Resolver for SameUnit {
    fn resolve(&self, from: &str, import: &Import) -> Option<Import> {
        let same = match self.target {
            TargetKind::Go => same_go_package(from, &import.module),
            TargetKind::Rust | TargetKind::TypeScript => from == import.module,
        };
        (!same).then(|| import.clone())
    }
}

/// Reject a Go layout that would emit source Go cannot compile, so the failure is
/// a clear error rather than silently-broken output. Go has no relative imports,
/// so an SDK whose modules import each other (or the shared internal package)
/// needs the module path (`--go-module`); and a package is named for its module's
/// last segment, so two modules sharing that segment, or one taking the shared
/// package's name, would render colliding selectors. Config is applied first, so
/// `--flatten` (which joins the whole path into one segment) clears the
/// collision. A no-op when Go is not a requested target.
pub fn check_layout(
    model: &Model,
    targets: &[TargetKind],
    config: &CodegenConfig,
) -> Result<(), String> {
    check_root_names(model, targets, config)?;
    if !targets.contains(&TargetKind::Go) {
        return Ok(());
    }
    let model = modules::apply(config, model);
    let names: Vec<&str> = model.modules.iter().map(|m| m.name.as_str()).collect();
    let shared = go_has_shared_package(&model);
    if go_needs_module_path(&model) && config.go_module.is_none() {
        return Err(
            "Go output with more than one package needs --go-module <path>: Go \
             has no relative imports, so a cross-package import needs the SDK's \
             module path"
                .into(),
        );
    }
    let mut by_pkg: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for name in &names {
        by_pkg.entry(package_name(name)).or_default().push(name);
    }
    if let Some((pkg, mods)) = by_pkg.iter().find(|(_, v)| v.len() > 1) {
        return Err(format!(
            "Go package name collision: modules {} both map to package '{pkg}'; \
             rename a module so its last segment is unique, or --flatten",
            mods.join(" and ")
        ));
    }
    if shared {
        if let Some(mods) = by_pkg.get(ROOT_UNIT) {
            return Err(format!(
                "Go package name collision: module {} maps to package \
                 '{ROOT_UNIT}', which the SDK's shared internal package \
                 takes; rename the module, or --flatten",
                mods.join(" and ")
            ));
        }
    }
    Ok(())
}

/// Reject a layout whose SDK-root groups collide with a module's own name.
///
/// Rust and TypeScript put those groups at the root of the output rather than
/// under a directory, so a module of the same name lands beside them: Rust reads
/// a `codec.rs` and a `codec/` as one module and refuses the crate, and a
/// TypeScript relative import of `./codec` resolves to the file, shadowing the
/// module's barrel. Go is unaffected, since its shared packages live under
/// `internal/` where no module can reach.
fn check_root_names(
    model: &Model,
    targets: &[TargetKind],
    config: &CodegenConfig,
) -> Result<(), String> {
    for target in [TargetKind::Rust, TargetKind::TypeScript] {
        if targets.contains(&target) {
            check_root_names_for(model, target, config)?;
        }
    }
    Ok(())
}

fn check_root_names_for(
    model: &Model,
    target: TargetKind,
    config: &CodegenConfig,
) -> Result<(), String> {
    let model = modules::apply(config, model);
    // Ask the assembler rather than restating which declarations pull a shared
    // group, so the check cannot drift from what actually gets written.
    let shared = crate::codegen::assemble::shared_files(
        &model,
        target,
        &crate::codegen::pipeline::casing_for(target),
    );
    if shared.is_empty() {
        return Ok(());
    }
    let roots: Vec<&str> = shared
        .iter()
        .map(|file| file.group.name.as_str())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let taken: Vec<String> = model
        .modules
        .iter()
        .map(|m| m.name.as_str())
        .filter_map(|name| {
            let head = name.split('.').next()?;
            roots
                .contains(&head)
                .then(|| format!("{name} (as '{head}')"))
        })
        .collect();
    if taken.is_empty() {
        return Ok(());
    }
    Err(format!(
        "{target:?} module name collision: module {} lands beside the SDK's own \
         shared module of that name ({}); rename the module, or --flatten",
        taken.join(" and "),
        roots.join(", ")
    ))
}

#[cfg(test)]
#[path = "layout_tests.rs"]
mod tests;

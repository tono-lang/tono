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
        // fence is the package's `exports` map plus what the barrel names. Only
        // the SDK-root plumbing gets a file of its own, under the `internal/`
        // subtree the ecosystem uses for it (rxjs, effect, openai-node); a
        // module's internal group rides the module's own file, since the barrel
        // is already the thing deciding its surface.
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
        (None, TargetKind::TypeScript) if grp.is_internal() => {
            root.join(INTERNAL_DIR).join(format!("{}.{ext}", grp.name))
        }
        (None, TargetKind::Rust) if grp.is_internal() => root.join(format!("{}.{ext}", grp.name)),
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
    check_rust_layout(model, targets, config)?;
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

/// Reject a Rust layout that would emit source Rust cannot compile. The SDK-root
/// group is a file at the crate root, and a module of the same name is a
/// directory beside it: Rust reads both as one module and refuses the crate. A
/// no-op when Rust is not a requested target, or when nothing shared is emitted.
fn check_rust_layout(
    model: &Model,
    targets: &[TargetKind],
    config: &CodegenConfig,
) -> Result<(), String> {
    if !targets.contains(&TargetKind::Rust) {
        return Ok(());
    }
    let model = modules::apply(config, model);
    // Ask the assembler rather than restating which fields pull a helper, so the
    // check cannot drift from what actually gets written.
    let shared = crate::codegen::assemble::shared_files(
        &model,
        TargetKind::Rust,
        &crate::codegen::targets::rust::types::rust_casing(),
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
        "Rust module name collision: module {} maps to a directory beside the \
         SDK's own shared module of that name ({}); rename the module, or \
         --flatten",
        taken.join(" and "),
        roots.join(", ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::generate;
    use crate::ir::{Module, Prim, Shape, ShapeKind, Tref};

    fn path_of(target: TargetKind, grp: &Group) -> String {
        output_path(target, grp)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/")
    }

    fn model(names: &[&str]) -> Model {
        Model {
            tono_ir_version: 6,
            modules: names
                .iter()
                .map(|name| Module {
                    name: (*name).into(),
                    shapes: vec![],
                    operations: vec![],
                    extensions: vec![],
                })
                .collect(),
        }
    }

    /// A model whose first module declares an entry, which is what puts anything
    /// in the shared package.
    fn model_with_entry(names: &[&str]) -> Model {
        let mut model = model(names);
        model.modules[0].shapes.push(crate::ir::Shape {
            id: format!("{}#client", names[0]),
            kind: crate::ir::ShapeKind::Entry {
                fields: vec![],
                operations: vec![],
            },
            traits: vec![],
        });
        model
    }

    #[test]
    fn go_maps_a_module_to_a_package_directory_and_each_group_to_a_file() {
        assert_eq!(
            path_of(TargetKind::Go, &Group::types("payments.common")),
            "go/payments/common/types.go"
        );
        assert_eq!(
            path_of(TargetKind::Go, &Group::entry("payments.charges", "client")),
            "go/payments/charges/client.go"
        );
        // Each SDK-root group is a package of its own under internal/, named for
        // what it holds.
        assert_eq!(
            path_of(TargetKind::Go, &Group::root_codec()),
            "go/internal/codec/codec.go"
        );
        assert_eq!(
            path_of(TargetKind::Go, &Group::root_config()),
            "go/internal/config/config.go"
        );
    }

    #[test]
    fn rust_and_typescript_map_every_group_to_a_file() {
        assert_eq!(
            path_of(TargetKind::Rust, &Group::types("payments.common")),
            "rust/payments/common/types.rs"
        );
        // Rust fences with visibility, so nothing moves and no file is named for
        // its audience: the SDK-root group is a module named for its contents,
        // and a module's internal group rides its public file as `pub(crate)`.
        assert_eq!(
            path_of(TargetKind::Rust, &Group::root_codec()),
            "rust/codec.rs"
        );
        assert_eq!(
            path_of(
                TargetKind::Rust,
                &Group::module_internal("payments.charges")
            ),
            path_of(TargetKind::Rust, &Group::types("payments.charges"))
        );
        // The codec cannot move (an impl lives where its type does), so it is
        // fenced where it sits.
        assert_eq!(
            path_of(TargetKind::Rust, &Group::codec("payments.charges")),
            "rust/payments/charges/codec.rs"
        );
        assert_eq!(
            path_of(
                TargetKind::TypeScript,
                &Group::module_internal("payments.charges")
            ),
            path_of(TargetKind::TypeScript, &Group::types("payments.charges"))
        );
        assert_eq!(
            path_of(TargetKind::TypeScript, &Group::root_codec()),
            "typescript/internal/codec.ts"
        );
        assert_eq!(
            path_of(TargetKind::TypeScript, &Group::root_config()),
            "typescript/internal/config.ts"
        );
        assert_eq!(
            target_relative_path(TargetKind::TypeScript, &Group::types("notes"))
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
            "notes/types.ts"
        );
    }

    #[test]
    fn go_groups_of_one_module_share_a_package() {
        assert!(same_go_package(
            "payments.charges::types",
            "payments.charges::codec"
        ));
        // The group Go moves under internal/ is a package of its own, which is
        // what puts it out of a consumer's reach.
        assert!(!same_go_package(
            "payments.charges::types",
            "payments.charges::internal"
        ));
        assert_eq!(
            path_of(TargetKind::Go, &Group::module_internal("payments.charges")),
            "go/internal/payments/charges/charges.go"
        );
        assert_eq!(
            go_import("example.com/sdk", "payments.charges::internal").as_deref(),
            Some("example.com/sdk/internal/payments/charges")
        );
        // The relocated package keeps the module's whole path, like the public
        // one: flattening to the last segment would land two modules that share
        // it on the same file.
        assert_ne!(
            path_of(TargetKind::Go, &Group::module_internal("payments.common")),
            path_of(TargetKind::Go, &Group::module_internal("billing.common"))
        );
        assert!(!same_go_package(
            "payments.charges::types",
            "payments.common::types"
        ));
        // The SDK-root group is its own package, and an external specifier is
        // never in anyone's package.
        assert!(!same_go_package("payments.charges::types", "::internal"));
        assert!(same_go_package("::internal", "::internal"));
        assert!(!same_go_package("payments.charges::types", "encoding/json"));
    }

    #[test]
    fn import_paths_follow_each_language_idiom() {
        assert_eq!(
            rust_path("payments.common::types").as_deref(),
            Some("crate::payments::common::types")
        );
        assert_eq!(rust_path("::codec").as_deref(), Some("crate::codec"));
        assert_eq!(rust_path("::config").as_deref(), Some("crate::config"));
        assert_eq!(
            rust_path("payments.charges::internal").as_deref(),
            Some("crate::payments::charges::types")
        );

        assert_eq!(
            go_import("example.com/sdk", "payments.common::types").as_deref(),
            Some("example.com/sdk/payments/common")
        );
        assert_eq!(
            go_import("example.com/sdk", "::codec").as_deref(),
            Some("example.com/sdk/internal/codec")
        );
        assert_eq!(go_import("example.com/sdk", "encoding/json"), None);
        assert_eq!(
            go_selector("payments.common::types").as_deref(),
            Some("common")
        );
        assert_eq!(go_selector("::codec").as_deref(), Some("codec"));
        assert_eq!(go_selector("::config").as_deref(), Some("config"));

        assert_eq!(
            ts_specifier("payments.charges::types", "payments.common::types").as_deref(),
            Some("../common/types")
        );
        assert_eq!(
            ts_specifier("payments.charges::types", "::codec").as_deref(),
            Some("../../internal/codec")
        );
        // A module's internal group shares its public file, so a reference to it
        // is a reference to that file.
        assert_eq!(
            ts_specifier("payments.charges::types", "payments.charges::internal").as_deref(),
            Some("./types")
        );
        assert_eq!(
            ts_specifier("payments.charges::types", "payments.charges::codec").as_deref(),
            Some("./codec")
        );
        assert_eq!(
            ts_specifier("notes::types", "::config").as_deref(),
            Some("../internal/config")
        );
        assert_eq!(ts_specifier("notes::types", "@tono/http-runtime-ts"), None);
    }

    #[test]
    fn the_shared_go_package_exists_only_when_something_uses_it() {
        // Its content is the entry construction helpers, so a model with no
        // entry declares nothing shared and stays a single package.
        assert!(!go_has_shared_package(&model(&["notes"])));
        assert!(go_has_shared_package(&model_with_entry(&["notes"])));
        // A second package means the cross-package imports need a module path.
        let err = check_layout(
            &model_with_entry(&["notes"]),
            &[TargetKind::Go],
            &CodegenConfig::default(),
        )
        .unwrap_err();
        assert!(err.contains("--go-module"));
    }

    /// A model whose first module has a wide-integer field, which is what puts
    /// anything in the Rust crate's shared serialization module.
    fn model_with_wide_int(names: &[&str]) -> Model {
        let mut model = model(names);
        model.modules[0]
            .shapes
            .push(crate::codegen::test_support::structure(
                &format!("{}#Charge", names[0]),
                vec![crate::codegen::test_support::member(
                    "amount",
                    Tref::Prim(Prim::I64),
                    true,
                )],
            ));
        model
    }

    #[test]
    fn a_rust_module_taking_the_shared_modules_name_is_rejected() {
        // The shared group is a file at the crate root and the module would be a
        // directory beside it; Rust reads both as one module and refuses the
        // crate, so the collision has to fail here with a name to act on.
        let collides = model_with_wide_int(&["codec", "notes"]);
        let err =
            check_layout(&collides, &[TargetKind::Rust], &CodegenConfig::default()).unwrap_err();
        assert!(err.contains("module name collision"), "got {err}");
        // Flatten joins the path into one segment, so a nested module clears it.
        let nested = model_with_wide_int(&["codec.charges"]);
        assert!(
            check_layout(&nested, &[TargetKind::Rust], &CodegenConfig::default()).is_err(),
            "a first segment is enough to collide: it is the directory Rust sees"
        );
        let flat = CodegenConfig {
            flatten: true,
            ..CodegenConfig::default()
        };
        assert!(check_layout(&nested, &[TargetKind::Rust], &flat).is_ok());
        // With nothing shared there is no file to collide with.
        assert!(check_layout(
            &model(&["codec"]),
            &[TargetKind::Rust],
            &CodegenConfig::default()
        )
        .is_ok());
        // And the name is Rust's alone: Go and TypeScript never emit it.
        assert!(check_layout(
            &model_with_wide_int(&["codec"]),
            &[TargetKind::TypeScript],
            &CodegenConfig::default()
        )
        .is_ok());
    }

    #[test]
    fn go_multi_module_without_a_module_path_is_rejected() {
        let model = model(&["payments.common", "payments.charges"]);
        let err = check_layout(&model, &[TargetKind::Go], &CodegenConfig::default()).unwrap_err();
        assert!(err.contains("--go-module"));
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        assert!(check_layout(&model, &[TargetKind::Go], &config).is_ok());
        // Rust and TypeScript have relative imports, so the same layout is fine.
        assert!(check_layout(
            &model,
            &[TargetKind::Rust, TargetKind::TypeScript],
            &CodegenConfig::default()
        )
        .is_ok());
    }

    #[test]
    fn go_modules_sharing_a_last_segment_are_rejected() {
        let model = model(&["a.common", "b.common"]);
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        let err = check_layout(&model, &[TargetKind::Go], &config).unwrap_err();
        assert!(err.contains("package name collision"));
        // Flatten joins the whole path into one segment, so the packages differ.
        let flat = CodegenConfig {
            flatten: true,
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        assert!(check_layout(&model, &[TargetKind::Go], &flat).is_ok());
    }

    #[test]
    fn the_shared_package_name_is_reserved_from_the_modules() {
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        let err = check_layout(
            &model_with_entry(&["tono", "notes"]),
            &[TargetKind::Go],
            &config,
        )
        .unwrap_err();
        assert!(err.contains("shared internal package"));
        // With nothing shared there is no such package, so the name is free.
        assert!(check_layout(&model(&["tono", "notes"]), &[TargetKind::Go], &config).is_ok());
    }

    /// A module declaring two entries, one of them named something other than
    /// `client`, so the layout has to name a group after each declaration.
    fn two_entry_model() -> Model {
        let field = crate::ir::EntryField {
            name: "endpoint".into(),
            target: Tref::Prim(Prim::String),
            sources: vec![],
            format: None,
            transforms: vec![],
            select: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![crate::ir::Trait {
                id: "arg".into(),
                value: serde_json::Value::Null,
            }],
        };
        let entry = |name: &str| Shape {
            id: format!("notes#{name}"),
            kind: ShapeKind::Entry {
                fields: vec![field.clone()],
                operations: vec![],
            },
            traits: vec![],
        };
        Model {
            tono_ir_version: 6,
            modules: vec![Module {
                name: "notes".into(),
                shapes: vec![entry("admin"), entry("reader")],
                operations: vec![],
                extensions: vec![],
            }],
        }
    }

    #[test]
    fn each_entry_declaration_gets_a_group_named_after_it() {
        for (target, ext) in [(TargetKind::Go, "go"), (TargetKind::TypeScript, "ts")] {
            let config = CodegenConfig {
                go_module: Some("example.com/sdk".into()),
                ..CodegenConfig::default()
            };
            let files = generate(&two_entry_model(), &[target], &config).unwrap();
            let paths = files
                .iter()
                .map(|f| {
                    f.path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                })
                .collect::<Vec<_>>();
            let dir = target.dir();
            // Two entries in one module are two groups, each named for its
            // declaration rather than for a fixed `client`.
            assert!(paths.contains(&format!("{dir}/notes/admin.{ext}")));
            assert!(paths.contains(&format!("{dir}/notes/reader.{ext}")));
            // Each holds its own constructor, not the other's.
            let admin = &files
                .iter()
                .find(|f| {
                    f.path
                        .to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/")
                        == format!("{dir}/notes/admin.{ext}")
                })
                .expect("the entry's own group")
                .text;
            assert!(admin.contains("Admin"));
            assert!(!admin.contains("Reader"));
        }
    }
}

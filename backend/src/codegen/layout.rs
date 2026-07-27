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

/// The Go package name of the SDK-root group, and its directory under
/// `internal/`. Go needs a package directory below `internal/`, and the name has
/// to be one no module can take (see [`check_go_layout`]).
pub(crate) const GO_ROOT_PACKAGE: &str = "tono";

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
/// makes a cross-package import (and so a module path) unavoidable: the shared
/// helpers, or any declaration Go moves under `internal/`.
pub fn go_needs_module_path(model: &Model) -> bool {
    if go_has_shared_package(model) || model.modules.len() > 1 {
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
        // Go has exactly one thing that fences a declaration off: the `internal/`
        // directory, which the toolchain refuses to resolve from outside the SDK.
        // So every group Go can move lands there, as a package of its own, and a
        // module's public package holds only files named for what they contain.
        // Rust and TypeScript need no relocation: a private module and an
        // unlisted subpath fence a file in place.
        (None, TargetKind::Go) => root
            .join("internal")
            .join(GO_ROOT_PACKAGE)
            .join(format!("{GO_ROOT_PACKAGE}.{ext}")),
        (None, _) => root.join(format!("{}.{ext}", grp.name)),
        (Some(module), TargetKind::Go) if grp.is_internal() && !grp.is_colocated() => {
            let package = package_name(module);
            root.join("internal")
                .join(package)
                .join(format!("{package}.{ext}"))
        }
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
    Some((module?, name == group::INTERNAL))
}

/// The Rust crate path of a group, or `None` when the path is not a group path
/// (an external crate or a standard-library module).
pub fn rust_path(path: &str) -> Option<String> {
    let (module, name) = group::parse_path(path)?;
    Some(match module {
        None => format!("crate::{name}"),
        Some(module) => format!("crate::{}::{name}", module.replace('.', "::")),
    })
}

/// The Go import path of a group under the SDK's module path, or `None` when the
/// path is not a group path.
pub fn go_import(go_module: &str, path: &str) -> Option<String> {
    let (module, name) = group::parse_path(path)?;
    Some(match module {
        None => format!("{go_module}/internal/{GO_ROOT_PACKAGE}"),
        Some(module) if name == group::INTERNAL => {
            format!("{go_module}/internal/{}", package_name(module))
        }
        Some(module) => format!("{go_module}/{}", module.replace('.', "/")),
    })
}

/// The Go package selector a group is referenced through (its directory's last
/// segment), or `None` when the path is not a group path.
pub fn go_selector(path: &str) -> Option<String> {
    let (module, _) = group::parse_path(path)?;
    Some(match module {
        None => GO_ROOT_PACKAGE.to_string(),
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
        let (module, name) = group::parse_path(path)?;
        Some(match module {
            None => PathBuf::from(name),
            Some(module) => module_dir(module).join(name),
        })
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
pub fn check_go_layout(
    model: &Model,
    targets: &[TargetKind],
    config: &CodegenConfig,
) -> Result<(), String> {
    if !targets.contains(&TargetKind::Go) {
        return Ok(());
    }
    let model = modules::apply(config, model);
    let names: Vec<&str> = model.modules.iter().map(|m| m.name.as_str()).collect();
    let shared = go_has_shared_package(&model);
    if go_needs_module_path(&model) && config.go_module.is_none() {
        return Err(
            "Go output with more than one package needs --go-module <path>: Go has \
             no relative imports, so a cross-package import needs the SDK's module \
             path"
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
        if let Some(mods) = by_pkg.get(GO_ROOT_PACKAGE) {
            return Err(format!(
                "Go package name collision: module {} maps to package \
                 '{GO_ROOT_PACKAGE}', which the SDK's shared internal package \
                 takes; rename the module, or --flatten",
                mods.join(" and ")
            ));
        }
    }
    Ok(())
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
        // Only the SDK-root group is a package of its own, under internal/.
        assert_eq!(
            path_of(TargetKind::Go, &Group::root_internal()),
            "go/internal/tono/tono.go"
        );
    }

    #[test]
    fn rust_and_typescript_map_every_group_to_a_file() {
        assert_eq!(
            path_of(TargetKind::Rust, &Group::types("payments.common")),
            "rust/payments/common/types.rs"
        );
        assert_eq!(
            path_of(TargetKind::Rust, &Group::root_internal()),
            "rust/internal.rs"
        );
        assert_eq!(
            path_of(
                TargetKind::TypeScript,
                &Group::module_internal("payments.charges")
            ),
            "typescript/payments/charges/internal.ts"
        );
        assert_eq!(
            path_of(TargetKind::TypeScript, &Group::root_internal()),
            "typescript/internal.ts"
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
            output_path(TargetKind::Go, &Group::module_internal("payments.charges"))
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
            "go/internal/charges/charges.go"
        );
        assert_eq!(
            go_import("example.com/sdk", "payments.charges::internal").as_deref(),
            Some("example.com/sdk/internal/charges")
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
        assert_eq!(rust_path("::internal").as_deref(), Some("crate::internal"));

        assert_eq!(
            go_import("example.com/sdk", "payments.common::types").as_deref(),
            Some("example.com/sdk/payments/common")
        );
        assert_eq!(
            go_import("example.com/sdk", "::internal").as_deref(),
            Some("example.com/sdk/internal/tono")
        );
        assert_eq!(go_import("example.com/sdk", "encoding/json"), None);
        assert_eq!(
            go_selector("payments.common::types").as_deref(),
            Some("common")
        );
        assert_eq!(go_selector("::internal").as_deref(), Some("tono"));

        assert_eq!(
            ts_specifier("payments.charges::types", "payments.common::types").as_deref(),
            Some("../common/types")
        );
        assert_eq!(
            ts_specifier("payments.charges::types", "::internal").as_deref(),
            Some("../../internal")
        );
        assert_eq!(
            ts_specifier("payments.charges::types", "payments.charges::internal").as_deref(),
            Some("./internal")
        );
        assert_eq!(
            ts_specifier("notes::types", "::internal").as_deref(),
            Some("../internal")
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
        let err = check_go_layout(
            &model_with_entry(&["notes"]),
            &[TargetKind::Go],
            &CodegenConfig::default(),
        )
        .unwrap_err();
        assert!(err.contains("--go-module"));
    }

    #[test]
    fn go_multi_module_without_a_module_path_is_rejected() {
        let model = model(&["payments.common", "payments.charges"]);
        let err =
            check_go_layout(&model, &[TargetKind::Go], &CodegenConfig::default()).unwrap_err();
        assert!(err.contains("--go-module"));
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        assert!(check_go_layout(&model, &[TargetKind::Go], &config).is_ok());
        // Rust and TypeScript have relative imports, so the same layout is fine.
        assert!(check_go_layout(
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
        let err = check_go_layout(&model, &[TargetKind::Go], &config).unwrap_err();
        assert!(err.contains("package name collision"));
        // Flatten joins the whole path into one segment, so the packages differ.
        let flat = CodegenConfig {
            flatten: true,
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        assert!(check_go_layout(&model, &[TargetKind::Go], &flat).is_ok());
    }

    #[test]
    fn the_shared_package_name_is_reserved_from_the_modules() {
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        let err = check_go_layout(
            &model_with_entry(&["tono", "notes"]),
            &[TargetKind::Go],
            &config,
        )
        .unwrap_err();
        assert!(err.contains("shared internal package"));
        // With nothing shared there is no such package, so the name is free.
        assert!(check_go_layout(&model(&["tono", "notes"]), &[TargetKind::Go], &config).is_ok());
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

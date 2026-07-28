//! The per-module re-export each target needs to make its groups reachable.
//!
//! Splitting a module across groups is only half the layout; the other half is
//! saying, once per module, what of it a consumer may reach. Rust declares the
//! module's groups and re-exports the public ones with `pub use`, leaving the
//! internal one a private `mod`. TypeScript emits the module's barrel and lists
//! only the barrels in the package's `exports` map, so a deep import of an
//! internal file is refused by the resolver. Go needs neither: a module is a
//! single package, so its groups are already one unit, and the SDK's shared
//! group is fenced off by `internal/` instead.
//!
//! There is deliberately no root barrel aggregating the modules: an SDK's
//! modules are separate surfaces, and flattening them into one namespace would
//! make every rename of a module a breaking change to a name that never moved.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::codegen::group::Group;
use crate::codegen::layout;
use crate::codegen::pipeline::{GeneratedFile, TargetKind, BANNER};
use crate::codegen::targets::typescript::emit::Exports;

/// One group's re-export lines: types and values are spelled apart, since a
/// build with `isolatedModules` refuses a type re-exported as a value. A group
/// that exports nothing contributes no line.
fn named_exports(group: &str, exports: &Exports) -> String {
    let line = |keyword: &str, names: &[String]| -> String {
        if names.is_empty() {
            return String::new();
        }
        format!(
            "export {keyword}{{ {} }} from \"./{group}\";\n",
            names.join(", ")
        )
    };
    format!(
        "{}{}",
        line("type ", &exports.types),
        line("", &exports.values)
    )
}

/// A directory's children in the Rust module tree.
#[derive(Default)]
struct RustDir {
    /// Child directories, each declared as a module of this one.
    dirs: BTreeSet<String>,
    /// Child files, as `(module name, public)`. Several groups can share one
    /// file (Rust fences a module's internal group with visibility rather than
    /// with a file of its own), and the file is declared for the widest audience
    /// among them.
    files: BTreeMap<String, bool>,
}

/// How a child module is declared: `pub mod`, or a private `mod` for an internal
/// group.
///
/// Rust fences where the module sits, so an internal group needs no relocation:
/// a `mod` without `pub` is unreachable from outside the crate and reachable
/// from inside it, which is exactly what internal means here.
fn declare(name: &str, fenced: bool) -> String {
    if fenced {
        // The declarations inside exist for the SDK's own use, so the lint that
        // wants every item reached is not the right judge of them. The allowance
        // covers the whole subtree below the fence.
        format!("#[allow(dead_code)]\nmod {name};\n")
    } else {
        format!("pub mod {name};\n")
    }
}

/// The Rust module tree: a `mod.rs` per directory of generated files and a
/// `lib.rs` at the crate root, declaring each child and re-exporting the public
/// groups so a consumer names `sdk::payments::Charge` rather than reaching into
/// the group a declaration happens to live in.
pub fn rust_module_tree(groups: &[Group]) -> Vec<GeneratedFile> {
    let root = PathBuf::from(TargetKind::Rust.dir());
    let mut dirs: BTreeMap<PathBuf, RustDir> = BTreeMap::new();
    dirs.entry(root.clone()).or_default();
    for group in groups {
        let path = layout::output_path(TargetKind::Rust, group);
        let parent = path.parent().unwrap_or(&root).to_path_buf();
        let stem = path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let public = !group.is_internal();
        dirs.entry(parent.clone())
            .or_default()
            .files
            .entry(stem)
            .and_modify(|seen| *seen |= public)
            .or_insert(public);
        // Register each directory as a child of the one above it, up to the
        // crate root, so an intermediate namespace directory is declared too.
        let mut child = parent;
        while let Some(above) = child.parent().map(PathBuf::from) {
            if child == root {
                break;
            }
            let name = child
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            dirs.entry(above.clone()).or_default().dirs.insert(name);
            child = above;
        }
    }
    dirs.into_iter()
        .map(|(dir, node)| {
            let mut body = String::new();
            for name in &node.dirs {
                // A directory is a module's namespace, never a group, so nothing
                // internal is expressed here.
                body.push_str(&declare(name, false));
            }
            for (name, public) in &node.files {
                body.push_str(&declare(name, !public));
            }
            // The crate root declares its children and stops there: flattening
            // them into one namespace is the root barrel this layout refuses,
            // and a consumer reaches a group by naming it.
            let uses: String = if dir == root {
                String::new()
            } else {
                node.files
                    .iter()
                    .filter(|(_, public)| **public)
                    .map(|(name, _)| format!("pub use {name}::*;\n"))
                    .collect()
            };
            if !uses.is_empty() {
                body.push('\n');
                body.push_str(&uses);
            }
            let name = if dir == root { "lib.rs" } else { "mod.rs" };
            GeneratedFile {
                target: TargetKind::Rust,
                path: dir.join(name),
                text: format!("{BANNER}{body}"),
            }
        })
        .collect()
}

/// The TypeScript barrels and the package manifest: one `index.ts` per module
/// re-exporting its public groups, and a `package.json` whose `exports` map lists
/// exactly those barrels, so an internal file has no specifier a consumer can
/// resolve.
pub fn typescript_barrels(groups: &[(Group, Exports)]) -> Vec<GeneratedFile> {
    let mut by_module: BTreeMap<&str, Vec<&(Group, Exports)>> = BTreeMap::new();
    let mut files = Vec::new();
    let mut exports: Vec<(String, String)> = Vec::new();
    for entry in groups {
        let group = &entry.0;
        match group.module.as_deref() {
            Some(module) => by_module.entry(module).or_default().push(entry),
            // A public root group is its own entry point: it holds no module's
            // declarations, so it needs no barrel, only a specifier.
            None if !group.is_internal() => {
                exports.push((format!("./{}", group.name), format!("./{}.ts", group.name)))
            }
            None => {}
        }
    }
    for (module, groups) in by_module {
        let dir = PathBuf::from(TargetKind::TypeScript.dir()).join(layout::module_dir(module));
        // Named one at a time rather than re-exported whole: a module's internal
        // group shares a file with its public one (TypeScript has no per-symbol
        // visibility, so the fence is what the barrel names), and a star would
        // carry it out with the rest.
        let body: String = groups
            .iter()
            .filter(|(group, _)| !group.is_internal())
            .map(|(group, exports)| named_exports(&group.name, exports))
            .collect();
        files.push(GeneratedFile {
            target: TargetKind::TypeScript,
            path: dir.join("index.ts"),
            text: format!("{BANNER}{body}"),
        });
        let sub = layout::module_dir(module)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        exports.push((format!("./{sub}"), format!("./{sub}/index.ts")));
    }
    let entries: Vec<String> = exports
        .iter()
        .map(|(key, value)| format!("    \"{key}\": \"{value}\""))
        .collect();
    files.push(GeneratedFile {
        target: TargetKind::TypeScript,
        path: PathBuf::from(TargetKind::TypeScript.dir()).join("package.json"),
        // JSON carries no comment, so this one file has no banner.
        text: format!("{{\n  \"exports\": {{\n{}\n  }}\n}}\n", entries.join(",\n")),
    });
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_at<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
        let want = path.replace('/', std::path::MAIN_SEPARATOR_STR);
        &files
            .iter()
            .find(|f| f.path.to_string_lossy() == want)
            .unwrap_or_else(|| panic!("no file at {path}"))
            .text
    }

    /// The groups paired with what they export, as the barrel builder takes
    /// them. `<name>Type` is a type and `make<Name>` a value, so a barrel has to
    /// spell the two apart.
    fn exporting(groups: Vec<Group>) -> Vec<(Group, Exports)> {
        groups
            .into_iter()
            .map(|group| {
                let cap = |s: &str| {
                    let mut c = s.chars();
                    c.next()
                        .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                };
                let name = cap(&group.name);
                let exports = Exports {
                    types: vec![format!("{name}Type")],
                    values: vec![format!("make{name}")],
                };
                (group, exports)
            })
            .collect()
    }

    fn payments() -> Vec<Group> {
        vec![
            Group::root("duration"),
            Group::types("payments.common"),
            Group::module_internal("payments.common"),
            Group::types("payments.charges"),
            Group::entry("payments.charges", "client"),
            Group::module_internal("payments.charges"),
        ]
    }

    #[test]
    fn the_crate_root_declares_its_children_and_keeps_the_shared_group_private() {
        let files = rust_module_tree(&payments());
        let lib = text_at(&files, "rust/lib.rs");
        assert!(lib.contains("pub mod payments;"));
        // The shared group is a private module named for its contents:
        // unreachable from outside the crate, reachable from inside it.
        assert!(lib.contains("mod duration;"));
        assert!(!lib.contains("pub mod duration;"));
        // No root barrel: the crate root declares its children and does not
        // flatten them into one namespace.
        assert!(!lib.contains("pub use"));
    }

    #[test]
    fn a_module_declares_its_groups_and_re_exports_the_public_ones() {
        let files = rust_module_tree(&payments());
        let charges = text_at(&files, "rust/payments/charges/mod.rs");
        assert!(charges.contains("pub mod types;"));
        assert!(charges.contains("pub mod client;"));
        assert!(charges.contains("pub use types::*;"));
        assert!(charges.contains("pub use client::*;"));
        // The internal group has no module of its own: it rides the public file
        // with crate visibility, so nothing here is named for an audience and
        // the public file is still declared and re-exported.
        assert!(!charges.contains("internal"));
        assert!(charges.contains("pub mod types;"));
        // The namespace directory above the modules only declares them.
        let namespace = text_at(&files, "rust/payments/mod.rs");
        assert!(namespace.contains("pub mod charges;"));
        assert!(namespace.contains("pub mod common;"));
    }

    #[test]
    fn a_barrel_per_module_re_exports_only_the_public_groups() {
        let files = typescript_barrels(&exporting(payments()));
        let barrel = text_at(&files, "typescript/payments/charges/index.ts");
        // Named one at a time, and types apart from values: the internal group
        // shares a file with the public one, so a star would carry it out too.
        assert!(barrel.contains("export type { TypesType } from \"./types\";"));
        assert!(barrel.contains("export { makeTypes } from \"./types\";"));
        assert!(barrel.contains("export type { ClientType } from \"./client\";"));
        assert!(!barrel.contains("export *"));
        assert!(!barrel.contains("Internal"));
    }

    #[test]
    fn the_package_exports_map_lists_the_barrels_and_nothing_else() {
        let mut groups = payments();
        groups.push(Group::root_support());
        let files = typescript_barrels(&exporting(groups));
        let manifest = text_at(&files, "typescript/package.json");
        // A public root group is an entry point of its own.
        assert!(manifest.contains("\"./support\": \"./support.ts\""));
        assert!(manifest.contains("\"./payments/charges\": \"./payments/charges/index.ts\""));
        assert!(manifest.contains("\"./payments/common\": \"./payments/common/index.ts\""));
        // Nothing internal is reachable, and there is no root entry point that
        // would aggregate the modules.
        assert!(!manifest.contains("internal"));
        assert!(!manifest.contains("\".\":"));
        assert!(serde_json::from_str::<serde_json::Value>(manifest).is_ok());
    }

    #[test]
    fn a_single_segment_module_still_gets_its_own_directory_and_barrel() {
        let groups = vec![
            Group::types("notes"),
            Group::entry("notes", "admin"),
            Group::module_internal("notes"),
        ];
        let rust = rust_module_tree(&groups);
        assert!(text_at(&rust, "rust/lib.rs").contains("pub mod notes;"));
        assert!(text_at(&rust, "rust/notes/mod.rs").contains("pub use admin::*;"));
        let ts = typescript_barrels(&exporting(groups));
        assert!(text_at(&ts, "typescript/notes/index.ts").contains("from \"./admin\";"));
        assert!(text_at(&ts, "typescript/package.json").contains("\"./notes\""));
    }
}

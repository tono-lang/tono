//! Where a target's generated files land on disk, and the Rust module tree that
//! ties a nested layout together.
//!
//! This is the output-layout half of module mapping; the name-mapping half (the
//! remap/flatten hooks) is in [`crate::codegen::modules`]. Given already-effective
//! (post-config) module names, it decides each file's path and synthesizes the
//! `mod.rs` files a nested Rust layout needs.

use std::path::PathBuf;

use crate::codegen::modules::{self, CodegenConfig};
use crate::codegen::pipeline::{GeneratedFile, TargetKind, BANNER};
use crate::ir::Model;

/// The Go package name / Rust module leaf for a dotted module: its last segment.
pub(crate) fn package_name(module: &str) -> &str {
    module.rsplit('.').next().unwrap_or(module)
}

/// Where a module's file lands. A single-segment module stays flat
/// (`rust/payments.rs`); a dotted module maps to an idiomatic sub-package: Rust
/// and TypeScript use the dotted path as a file path (`rust/payments/common.rs`),
/// while Go nests the file inside a package directory named for the last segment
/// (`go/payments/common/common.go`), matching Go's dir-is-package rule.
pub(crate) fn output_path(target: TargetKind, module: &str, suffix: &str, ext: &str) -> PathBuf {
    let dir = PathBuf::from(target.dir());
    let segments: Vec<&str> = module.split('.').collect();
    if segments.len() == 1 {
        return dir.join(format!("{module}{suffix}.{ext}"));
    }
    match target {
        TargetKind::Go => dir
            .join(segments.join("/"))
            .join(format!("{}{suffix}.{ext}", package_name(module))),
        TargetKind::Rust | TargetKind::TypeScript => {
            dir.join(format!("{}{suffix}.{ext}", segments.join("/")))
        }
    }
}

/// Synthesize the Rust module tree: for every directory below the `rust/` root
/// that holds generated files, a `mod.rs` declaring `pub mod <child>;` for each
/// child module file (its stem) and subdirectory. The crate root's immediate
/// children (the top-level files or directories directly under `rust/`) are
/// declared by the consuming crate's `lib.rs`, so no `rust/mod.rs` is emitted; a
/// flat single-segment layout therefore produces no `mod.rs` at all.
pub(crate) fn rust_mod_tree(files: &[GeneratedFile]) -> Vec<GeneratedFile> {
    use std::collections::{BTreeMap, BTreeSet};

    let root = PathBuf::from(TargetKind::Rust.dir());
    let mut children: BTreeMap<PathBuf, BTreeSet<String>> = BTreeMap::new();
    for file in files.iter().filter(|f| f.target == TargetKind::Rust) {
        let comps: Vec<String> = file
            .path
            .iter()
            .map(|c| c.to_string_lossy().into_owned())
            .collect();
        // Register each path element as a child of the directory that holds it,
        // skipping the crate root (its children are the consumer's to declare).
        for i in 1..comps.len() {
            let parent: PathBuf = comps[..i].iter().collect();
            if parent == root {
                continue;
            }
            let raw = &comps[i];
            let child = if i == comps.len() - 1 {
                raw.strip_suffix(".rs").unwrap_or(raw)
            } else {
                raw
            };
            children
                .entry(parent)
                .or_default()
                .insert(child.to_string());
        }
    }
    children
        .into_iter()
        .map(|(dir, names)| {
            let body: String = names.iter().map(|n| format!("pub mod {n};\n")).collect();
            GeneratedFile {
                target: TargetKind::Rust,
                path: dir.join("mod.rs"),
                text: format!("{BANNER}{body}"),
            }
        })
        .collect()
}

/// Reject a Go layout that would emit source Go cannot compile, so the failure is
/// a clear error rather than silently-broken output. Go has no relative imports,
/// so a multi-module SDK's cross-package imports need the module path
/// (`--go-module`); and a package is named for its module's last segment, so two
/// modules sharing that segment would render colliding `pkg.` selectors. Config is
/// applied first, so `--flatten` (which joins the whole path into one segment)
/// clears both conditions. A no-op when Go is not a requested target.
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
    if names.len() > 1 && config.go_module.is_none() {
        return Err(
            "Go multi-module output needs --go-module <path>: Go has no relative \
             imports, so the cross-package imports need the SDK's module path"
                .into(),
        );
    }
    let mut by_pkg: std::collections::BTreeMap<&str, Vec<&str>> = std::collections::BTreeMap::new();
    for name in &names {
        by_pkg
            .entry(name.rsplit('.').next().unwrap_or(name))
            .or_default()
            .push(name);
    }
    if let Some((pkg, mods)) = by_pkg.iter().find(|(_, v)| v.len() > 1) {
        return Err(format!(
            "Go package name collision: modules {} both map to package '{pkg}'; \
             rename a module so its last segment is unique, or --flatten",
            mods.join(" and ")
        ));
    }
    Ok(())
}

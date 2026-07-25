//! Where a target's generated files land on disk, and the Rust module tree that
//! ties a nested layout together.
//!
//! This is the output-layout half of module mapping; the name-mapping half (the
//! remap/flatten hooks) is in [`crate::codegen::modules`]. Given already-effective
//! (post-config) module names, it decides each file's path and synthesizes the
//! `mod.rs` files a nested Rust layout needs.

use std::path::PathBuf;

use crate::codegen::pipeline::{GeneratedFile, TargetKind, BANNER};

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

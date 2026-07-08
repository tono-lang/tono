//! Codegen snapshot review gate. The emitted source for the shared wire-matrix
//! module (types, serde, error surface, and client) is snapshotted per target.
//! An unexpected diff fails the test; an intended change is accepted with
//! `cargo insta review`, so a human signs off on every codegen change.
//!
//! Rendering goes through the same path the conformance test uses, with a `cat`
//! formatter so the snapshot is hermetic (no language formatter need be installed)
//! and captures the emitter's own deterministic output.

use insta::assert_snapshot;
use tono_backend::codegen::render::render_file_with_companion;
use tono_backend::codegen::target::RenderRules;
use tono_backend::codegen::targets::{go, rust, typescript};
use tono_backend::codegen::tree::ModuleFile;
use tono_backend::codegen::Formatter;

mod common;
use common::matrix_module;

/// Render every emitted file of a module to one snapshot body: each file rendered
/// verbatim (identity formatter, so the gate needs no language formatter), tagged
/// with a `models<suffix>.<ext>` banner, and optionally prefixed by a `header`
/// (the Go package clause). The order of `files` is the emitter's own.
fn render_files(
    files: Vec<ModuleFile>,
    rules: &dyn RenderRules,
    ext: &str,
    header: Option<&str>,
) -> String {
    let identity = Formatter::new("cat", vec![]);
    let mut out = String::new();
    for module_file in files {
        let body = render_file_with_companion(
            &module_file.file,
            module_file.imports_companion.as_deref(),
            rules,
            &identity,
        )
        .text;
        let text = match header {
            Some(header) => format!("{header}\n{body}"),
            None => body,
        };
        out.push_str(&format!(
            "// ==== models{}.{ext} ====\n{text}\n",
            module_file.suffix
        ));
    }
    out
}

#[test]
fn rust_codegen_snapshot() {
    let files = rust::emit::emit_module(&matrix_module(), &rust::types::rust_casing());
    assert_snapshot!("rust", render_files(files, &rust::RustRules, "rs", None));
}

#[test]
fn go_codegen_snapshot() {
    let module = matrix_module();
    // The Go emitter needs the model's union ids so a struct field whose type is a
    // union gets a container UnmarshalJSON; for a single module it is the module's
    // own unions.
    let union_ids: std::collections::HashSet<String> = module
        .shapes
        .iter()
        .filter(|s| matches!(s.kind, tono_backend::ir::ShapeKind::Union { .. }))
        .map(|s| s.id.clone())
        .collect();
    let files = go::emit::emit_module(&module, &go::types::go_casing(), &union_ids);
    let header = go::emit::package_clause("models");
    assert_snapshot!(
        "go",
        render_files(files, &go::GoRules::default(), "go", Some(&header))
    );
}

#[test]
fn typescript_codegen_snapshot() {
    let files = typescript::emit::emit_module(&matrix_module(), &typescript::types::ts_casing());
    assert_snapshot!(
        "typescript",
        render_files(files, &typescript::TsRules, "ts", None)
    );
}

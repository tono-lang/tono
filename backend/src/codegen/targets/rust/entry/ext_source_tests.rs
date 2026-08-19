//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! the receiver borrowed back off the draft's `Option` slot (diagnosed, not
//! unwrapped, when unset), the awaited call, and the `yields`/`returns`
//! projection assigned into the field. The end-to-end `cargo build` proof
//! is `rust_ext_roundtrip::
//! a_field_sourced_from_a_handle_method_compiles_against_the_real_crate`;
//! this module exercises the emitter's own branches directly.

use crate::codegen::fixtures::handle_source::{handle_source_model, handle_source_module};
use crate::codegen::pipeline::generate_target;
use crate::codegen::targets::rust::types::rust_casing;
use crate::codegen::{CodegenConfig, TargetKind};
use crate::ir::{Model, ShapeKind, TONO_IR_VERSION};

fn entry_text(model: &Model) -> String {
    let files = generate_target(
        model,
        TargetKind::Rust,
        &CodegenConfig::default(),
        &rust_casing(),
    )
    .expect("the fixture model must generate cleanly");
    files
        .iter()
        .map(|f| f.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_field_borrows_the_receiver_awaits_the_call_and_projects_the_yields() {
    let out = entry_text(&handle_source_model("rust"));
    assert!(out.contains("let recv = match &s.provider {"), "{out}");
    assert!(
        out.contains("provider.get: the provider handle is not configured"),
        "{out}"
    );
    assert!(out.contains("let outcome = recv.get().await;"), "{out}");
    assert!(
        out.contains("Ok(c) => { s.config = Cfg { endpoint_read: c.read_url, endpoint_write: c.write_url }; }"),
        "{out}"
    );
    assert!(
        out.contains("contract_name: \"provider.get\".to_string()"),
        "{out}"
    );
    // The argument is cloned off the draft (never moved), and the op reading
    // the same handle still renders off the client.
    assert!(
        out.contains("let outcome = recv.get_for((s.region).clone()).await;"),
        "{out}"
    );
    assert!(
        out.contains("match recv.get_for((self.settings.region).clone()).await {"),
        "{out}"
    );
    assert!(
        out.contains("pub async fn build(self) -> Result<Client, TonoError> {"),
        "{out}"
    );
}

#[test]
fn a_sync_method_without_yields_assigns_the_bare_result() {
    let mut module = handle_source_module("rust");
    for method in &mut module.ext_libs[0].types[0].methods {
        method.langs[0].sync = true;
        method.langs[0].yields.clear();
        method.langs[0].returns = None;
    }
    let out = entry_text(&Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    });
    assert!(out.contains("let outcome = recv.get();"), "{out}");
    assert!(out.contains("Ok(v) => { s.config = v; }"), "{out}");
}

#[test]
fn an_entry_whose_only_call_is_a_handle_method_source_still_constructs_asynchronously() {
    let mut module = handle_source_module("rust");
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            let provider = fields.iter_mut().find(|f| f.name == "provider").unwrap();
            provider.call = None;
            provider.sources = vec![crate::ir::Source::Arg];
        }
    }
    let out = entry_text(&Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    });
    assert!(
        out.contains("pub async fn build(self) -> Result<Client, TonoError> {"),
        "{out}"
    );
    assert!(out.contains("let outcome = recv.get().await;"), "{out}");
}

//! Unit tests for the success-payload decode builders, split from
//! `entry/tests.rs` to stay within the file-size gate.

use super::*;
use crate::codegen::targets::rust::RustRules;
use crate::codegen::test_support::{member, rendered, structure};
use crate::ir::{Module, Prim, Shape, Tref};

fn charge_shape() -> Shape {
    structure(
        "m#charge",
        vec![member("id", Tref::Prim(Prim::String), true)],
    )
}

fn module_of(shapes: Vec<Shape>) -> Module {
    Module {
        name: "m".into(),
        shapes,
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
        tests: vec![],
    }
}

#[test]
fn success_block_with_no_required_members_skips_the_presence_probe() {
    let module = module_of(vec![structure(
        "m#note",
        vec![member("text", Tref::Prim(Prim::String), false)],
    )]);
    let out = success_block(
        Some(&Tref::Ref {
            id: "m#note".into(),
            args: vec![],
        }),
        &module,
        "body",
    );
    assert!(!out.contains("probe"));
    assert!(out.contains("serde_json::from_str::<Note>(body).map_err(|_| "));
}

#[test]
fn success_block_with_required_members_calls_the_shared_per_type_decode() {
    // The probe lives once per type (`output_decode_decl`), not once per call
    // site: the call site here is just the call.
    let out = success_block(
        Some(&Tref::Ref {
            id: "m#charge".into(),
            args: vec![],
        }),
        &module_of(vec![charge_shape()]),
        "body",
    );
    assert_eq!(out, "decode_charge(body)");
}

#[test]
fn output_decode_decl_probes_every_required_member_before_the_typed_decode() {
    let decl = output_decode_decl(&charge_shape()).expect("charge has a required member");
    let text = rendered(&[decl], &RustRules::default());
    assert!(text.contains("const CHARGE_REQUIRED_FIELDS: &[&str] = &[\"id\"];"));
    assert!(text.contains("fn decode_charge(body: &str) -> Result<Charge, TonoError> {"));
    assert!(text.contains("for field in CHARGE_REQUIRED_FIELDS {"));
    assert!(text.contains("if probe.get(field).map(|v| v.is_null()).unwrap_or(true) {"));
    assert!(text.contains("path: format!(\"$.{field}\")"));
    assert!(text.contains("serde_json::from_str::<Charge>(body).map_err("));
}

#[test]
fn output_decode_decl_skips_a_shape_with_no_required_member() {
    let shape = structure(
        "m#note",
        vec![member("text", Tref::Prim(Prim::String), false)],
    );
    assert!(output_decode_decl(&shape).is_none());
}

#[test]
fn success_block_of_a_bare_i64_output_parses_the_wire_string() {
    let module = module_of(vec![]);
    let out = success_block(Some(&Tref::Prim(Prim::I64)), &module, "body");
    assert!(out.contains("let wire: String = serde_json::from_str(body)"));
    assert!(out.contains("wire.parse::<i64>()"));
}

#[test]
fn success_block_of_a_bare_u64_output_parses_the_wire_string() {
    let module = module_of(vec![]);
    let out = success_block(Some(&Tref::Prim(Prim::U64)), &module, "body");
    assert!(out.contains("let wire: String = serde_json::from_str(body)"));
    assert!(out.contains("wire.parse::<u64>()"));
}

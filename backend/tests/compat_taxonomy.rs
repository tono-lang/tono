//! Detecting a pruned error-taxonomy declaration between two versions: a
//! category a module's generated code could construct in the baseline but no
//! longer can in the current model is a source break for whichever target
//! that flip applies to (a consumer that referenced the removed type, e.g.
//! caught `ContractError` specifically, stops compiling).
//!
//! Separate from `compat.rs` so each file stays within the repo size gate.

use tono_backend::compat::{diff, Category};
use tono_backend::ir::{
    EntryField, ExtKind, Extension, Prim, Shape, ShapeKind, TemplatePart, Tref, WireBinding,
    WireValue,
};

fn model(shapes: Vec<Shape>, extensions: Vec<Extension>) -> tono_backend::ir::Model {
    tono_backend::ir::Model {
        tono_ir_version: 6,
        modules: vec![tono_backend::ir::Module {
            tests: vec![],
            name: "notes".into(),
            shapes,
            operations: vec![],
            extensions,
        }],
    }
}

fn wire_op(id: &str) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: Some(Box::new(WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
                bindings: Default::default(),
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
        },
        traits: vec![],
    }
}

fn entry(id: &str, operations: Vec<Shape>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Entry {
            fields: vec![EntryField {
                name: "endpoint".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            }],
            operations,
        },
        traits: vec![],
    }
}

/// A `before_request` hook bound for TypeScript only, so the flip this drives
/// is isolated to the `typescript` target: the binding table only has a
/// `typescript` key, so `bound_extensions` finds nothing for `go` in either
/// model (Go's Contract liveness does not move). Rust's Contract liveness for
/// a wire-op entry is always true regardless of this binding (see
/// `taxonomy::derive_rust_entry`), so it does not move either, for a
/// different reason.
fn ts_before_request_hook() -> Extension {
    Extension {
        name: "before_request".into(),
        kind: ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("typescript".to_string(), "ext/ts/a.ts#hook".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }
}

#[test]
fn dropping_the_only_bound_hook_prunes_contract_for_typescript_only() {
    let baseline = model(
        vec![entry("notes#client", vec![wire_op("notes#client.ping")])],
        vec![ts_before_request_hook()],
    );
    let current = model(
        vec![entry("notes#client", vec![wire_op("notes#client.ping")])],
        vec![],
    );
    let report = diff(&baseline, &current);
    let pruned: Vec<&tono_backend::compat::Change> = report
        .changes
        .iter()
        .filter(|c| c.key.starts_with("prune-declaration"))
        .collect();
    assert_eq!(
        pruned.len(),
        1,
        "expected exactly one pruned category, got {pruned:?}"
    );
    let change = pruned[0];
    assert_eq!(
        change.key,
        "prune-declaration notes@typescript#ContractError"
    );
    assert_eq!(change.category, Category::SourceBreaking);
}

#[test]
fn a_module_with_nothing_pruned_reports_no_taxonomy_change() {
    let baseline = model(
        vec![entry("notes#client", vec![wire_op("notes#client.ping")])],
        vec![ts_before_request_hook()],
    );
    let current = baseline.clone();
    let report = diff(&baseline, &current);
    assert!(!report
        .changes
        .iter()
        .any(|c| c.key.starts_with("prune-declaration")));
}

#[test]
fn a_removed_module_is_not_double_reported_by_the_taxonomy_pass() {
    // The module's own removal (and its declared errors, if any) is already
    // reported by the shape-level diff; the taxonomy pass only compares
    // modules present on both sides, so it must add nothing extra here.
    let baseline = model(
        vec![entry("notes#client", vec![wire_op("notes#client.ping")])],
        vec![ts_before_request_hook()],
    );
    let current = tono_backend::ir::Model {
        tono_ir_version: 6,
        modules: vec![],
    };
    let report = diff(&baseline, &current);
    assert!(!report
        .changes
        .iter()
        .any(|c| c.key.starts_with("prune-declaration")));
}

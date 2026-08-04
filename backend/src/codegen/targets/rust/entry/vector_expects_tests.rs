//! Emitter tests for the expectation half of the generated `cargo test`
//! files: every outcome-pattern arm (equality, open/closed struct and error
//! patterns, the taxonomy categories with and without fields), the request
//! marker headers, and the defensive arms validation keeps out of the
//! pipeline. The declared tests come from the shared bed; only the assertions
//! over the generated Rust live here.

use std::collections::BTreeMap;

use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::targets::rust::{rust_casing, RustRules};
use crate::codegen::test_support::{absent, charge_bed, eq, present, rendered, with_tests};
use crate::ir::{FieldPattern, Module, ShapePattern, TaxonomyPattern, TestPattern};

fn fixture_module() -> Module {
    super::super::tests::simple_entry_module()
}

fn hermetic_text(module: &Module) -> String {
    let files = super::test_files(module, &rust_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &RustRules::default())
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let text = hermetic_text(&with_tests(fixture_module(), vec![charge_bed().ok_test()]));
    assert!(text.contains("result.map(|_| ()).expect(\"want ok\");"));
    assert!(!text.contains("serde_json::to_value"));
}

#[test]
fn a_closed_all_eq_struct_pattern_collapses_into_one_total_comparison() {
    let text = hermetic_text(&with_tests(
        fixture_module(),
        vec![charge_bed().closed_eq_struct_test()],
    ));
    assert!(text.contains("let out = result.expect(\"want ok\");"));
    assert!(text.contains("serde_json::from_str(r#\"{\"id\":\"c1\",\"tag\":\"t\"}\"#)"));
    assert!(text.contains("assert_eq!(got, want, \"output mismatch\");"));
}

#[test]
fn an_open_struct_pattern_checks_fields_and_markers_over_the_wire_form() {
    let text = hermetic_text(&with_tests(
        fixture_module(),
        vec![charge_bed().open_struct_test()],
    ));
    assert!(text.contains("let got = serde_json::to_value(&out).expect(\"encode output\");"));
    assert!(text.contains("assert_eq!(got.get(\"id\"), Some(&want), \"field id\");"));
    assert!(
        text.contains("assert!(got.get(\"extra\").is_some(), \"field extra must be present\");")
    );
    assert!(
        text.contains("assert!(got.get(\"missing\").is_none(), \"field missing must be absent\");")
    );
    // Open: unmentioned keys pass.
    assert!(!text.contains("unexpected field"));
}

#[test]
fn a_closed_struct_pattern_with_a_marker_rejects_unmentioned_keys() {
    let text = hermetic_text(&with_tests(
        fixture_module(),
        vec![charge_bed().closed_marker_struct_test()],
    ));
    assert!(text.contains("if let Some(object) = got.as_object() {"));
    assert!(text.contains(
        "assert!(matches!(key.as_str(), \"id\" | \"tag\"), \"unexpected field {key}\");"
    ));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let text = hermetic_text(&with_tests(fixture_module(), charge_bed().error_suite()));
    // The failure must be the declared typed error, unwrapped in two steps.
    assert!(text.contains("Err(TonoError::Api(failure)) => failure,"));
    assert!(text.contains("APIFailure::PaymentDeclined(data) => data,"));
    assert!(text.contains("panic!(\"want the declared error payment_declined, got {other:?}\"),"));
    // Open with fields: per-field checks over the re-encoded error data.
    assert!(
        text.contains("let got = serde_json::to_value(&declared).expect(\"encode error data\");")
    );
    assert!(text.contains("assert_eq!(got.get(\"message\"), Some(&want), \"field message\");"));
    assert!(text
        .contains("assert!(got.get(\"message\").is_some(), \"field message must be present\");"));
    assert!(
        text.contains("assert!(got.get(\"message\").is_none(), \"field message must be absent\");")
    );
    // Closed with only eq fields: one total comparison of the error data.
    assert!(text.contains("serde_json::from_str(r#\"{\"message\":\"busy\"}\"#)"));
    assert!(text.contains("assert_eq!(got, want, \"error data mismatch\");"));
    // A bare open pattern stops at the typed unwrap.
    assert!(text.contains("let _declared = match failure {"));
}

#[test]
fn taxonomy_patterns_cover_every_category_with_and_without_fields() {
    let bed = charge_bed();
    let mut tests = bed.taxonomy_suite();
    tests.extend(bed.taxonomy_bare_suite());
    let text = hermetic_text(&with_tests(fixture_module(), tests));
    assert!(text.contains("Err(TonoError::Api(APIFailure::Undeclared(e))) => e,"));
    assert!(text.contains("assert_eq!(api.status, 500, \"api status\");"));
    assert!(text.contains("assert_eq!(api.body, r#\"boom\"#, \"api body\");"));
    assert!(text.contains("let _ = api;"));
    assert!(text.contains("Err(TonoError::Validation(e)) => e,"));
    assert!(text.contains("assert_eq!(violated, vec![\"id\"], \"violated fields\");"));
    assert!(text.contains("let _ = invalid;"));
    assert!(text
        .contains("Err(TonoError::Decode(e)) => assert_eq!(e.path, \"$.id\", \"decode path\"),"));
    assert!(text.contains("Err(TonoError::Decode(e)) => drop(e),"));
    assert!(text.contains(
        "Err(TonoError::Contract(e)) => assert_eq!(e.contract_name, \"create_charge\", \"contract name\"),"
    ));
    assert!(text.contains("Err(TonoError::Contract(e)) => drop(e),"));
    assert!(text.contains(
        "Err(TonoError::Config(e)) => assert!(e.message.contains(\"api_key\"), \"config error names {}\", e.message),"
    ));
    assert!(text.contains("Err(TonoError::Config(e)) => drop(e),"));
    assert!(text.contains("Err(TonoError::Transport(_)) => {}"));
}

#[test]
fn request_header_markers_check_presence_and_absence() {
    let text = hermetic_text(&with_tests(
        fixture_module(),
        vec![charge_bed().request_marker_test()],
    ));
    assert!(text.contains("assert_eq!(seen.len(), 1, \"recorded requests\");"));
    assert!(text.contains(
        "assert!(lower0.contains_key(\"x-trace\"), \"request 0 header X-Trace must be present\");"
    ));
    assert!(text.contains(
        "assert!(!lower0.contains_key(\"x-debug\"), \"request 0 header X-Debug must be absent\");"
    ));
}

#[test]
fn the_field_leaf_readers_accept_equality_only() {
    assert_eq!(expects::eq_str(&eq(serde_json::json!("s"))), Some("s"));
    assert!(expects::eq_str(&eq(serde_json::json!(1))).is_none());
    assert!(expects::eq_str(&absent()).is_none());
    assert!(expects::eq_value(&present()).is_none());
    assert_eq!(
        expects::eq_value(&eq(serde_json::json!(7))),
        Some(&serde_json::json!(7))
    );
}

#[test]
fn map_field_asserts_skips_a_nested_structural_pattern() {
    let bed = charge_bed();
    let nested = FieldPattern::Pat(TestPattern::Struct(bed.struct_pattern(true, vec![])));
    let pattern = bed.struct_pattern(true, vec![("nested", nested)]);
    assert_eq!(expects::map_field_asserts(&pattern), "");
}

#[test]
fn a_closed_pattern_with_no_fields_rejects_every_key() {
    // Both emitter callers collapse a closed all-eq pattern (and the empty
    // field map is trivially all-eq) before reaching the per-field spelling,
    // so the no-arms rejection is pinned here at the unit seam.
    let text = expects::map_field_asserts(&charge_bed().struct_pattern(false, vec![]));
    assert!(text.contains("panic!(\"unexpected field {key}\");"));
}

#[test]
fn an_unknown_taxonomy_category_fails_loudly_in_the_generated_test() {
    let pattern = TaxonomyPattern {
        category: "bogus".into(),
        open: true,
        fields: BTreeMap::new(),
    };
    let text = expects::taxonomy_asserts(&pattern);
    assert!(text.contains("panic!(\"unknown error category bogus\");"));
}

/// A `TestCtx` over the fixture's first (valid) planned test, for the
/// defensive arms the shared validation keeps out of the pipeline.
fn with_ctx(module: &Module, f: impl FnOnce(&super::TestCtx<'_>)) {
    let (entries, multi, _bound) =
        plan::entry_setup(module, &super::BINDING_LANGS).expect("the fixture has an entry");
    let planned = declared_tests::entry_tests(module).expect("the declared tests validate");
    let entry = &entries[0];
    let n = super::super::names(entry, multi);
    let config = rust_casing();
    let ctx = super::TestCtx {
        entry,
        n: &n,
        module,
        config: &config,
        multi,
        test: &planned[0].tests[0],
    };
    f(&ctx);
}

#[test]
fn an_unknown_error_shape_fails_loudly_in_the_generated_test() {
    let module = with_tests(fixture_module(), vec![charge_bed().ok_test()]);
    with_ctx(&module, |ctx| {
        let text = expects::error_asserts(
            ctx,
            &ShapePattern {
                shape: "nope".into(),
                open: true,
                fields: BTreeMap::new(),
            },
        );
        assert!(text.contains("panic!(\"pattern names unknown error shape nope\");"));
    });
}

#[test]
fn an_eq_pattern_without_an_output_degrades_to_the_ok_assert() {
    // Validation ties an eq pattern to an op with an output, so the emitter's
    // no-output fallback is pinned here at the unit seam.
    let bed = charge_bed();
    let module = with_tests(
        fixture_module(),
        vec![bed.outcome_test("eq outcome", TestPattern::Eq(bed.input.clone()))],
    );
    with_ctx(&module, |ctx| {
        let text = expects::outcome_asserts(ctx, false);
        assert_eq!(text, "    result.map(|_| ()).expect(\"want ok\");\n");
    });
}

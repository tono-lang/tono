//! Emitter tests for the expectation half of the generated `cargo test`
//! files: every outcome-pattern arm (equality, open/closed struct and error
//! patterns, the taxonomy categories with and without fields), the request
//! marker headers, and the defensive arms validation keeps out of the
//! pipeline.

use std::collections::BTreeMap;

use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::targets::rust::{rust_casing, RustRules};
use crate::codegen::test_support::rendered;
use crate::ir::{
    Empty, FieldPattern, HttpAnswer, Module, RequestPattern, ShapePattern, StubAnswer, StubDep,
    TaxonomyPattern, TestCall, TestConstruction, TestDecl, TestExpect, TestPattern, TestStub,
};

fn fixture_module() -> Module {
    super::super::tests::simple_entry_module()
}

fn eq(value: serde_json::Value) -> FieldPattern {
    FieldPattern::Pat(TestPattern::Eq(value))
}

fn present() -> FieldPattern {
    FieldPattern::Present { present: Empty {} }
}

fn absent() -> FieldPattern {
    FieldPattern::Absent { absent: Empty {} }
}

fn construction() -> TestConstruction {
    TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([("api_key".to_string(), serde_json::json!("k"))]),
    }
}

fn call() -> TestCall {
    TestCall {
        binding: "got".into(),
        client: "c".into(),
        op: "create_charge".into(),
        input: Some(serde_json::json!({"id": "c1"})),
    }
}

fn http_stub() -> TestStub {
    TestStub {
        binding: None,
        client: "c".into(),
        op: "create_charge".into(),
        dep: StubDep::Http,
        answers: vec![StubAnswer::Http(HttpAnswer {
            status: 200,
            headers: BTreeMap::new(),
            body: "{\"id\":\"c1\"}".into(),
        })],
    }
}

fn outcome_test(name: &str, pattern: TestPattern) -> TestDecl {
    TestDecl {
        name: name.into(),
        constructions: vec![construction()],
        stubs: vec![http_stub()],
        calls: vec![call()],
        expects: vec![TestExpect::Outcome {
            subject: "got".into(),
            pattern,
        }],
    }
}

fn struct_pattern(open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
    ShapePattern {
        shape: "charge".into(),
        open,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

fn error_pattern(open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
    ShapePattern {
        shape: "payment_declined".into(),
        open,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

fn module_with(tests: Vec<TestDecl>) -> Module {
    let mut module = fixture_module();
    module.tests = tests;
    module
}

fn hermetic_text(module: &Module) -> String {
    let files = super::test_files(module, &rust_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &RustRules::default())
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let module = module_with(vec![outcome_test("just works", TestPattern::Ok(Empty {}))]);
    let text = hermetic_text(&module);
    assert!(text.contains("result.map(|_| ()).expect(\"want ok\");"));
    assert!(!text.contains("serde_json::to_value"));
}

#[test]
fn a_closed_all_eq_struct_pattern_collapses_into_one_total_comparison() {
    let module = module_with(vec![outcome_test(
        "pins the wire object",
        TestPattern::Struct(struct_pattern(
            false,
            vec![
                ("id", eq(serde_json::json!("c1"))),
                ("note", eq(serde_json::json!("n"))),
            ],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("let out = result.expect(\"want ok\");"));
    assert!(text.contains("serde_json::from_str(r#\"{\"id\":\"c1\",\"note\":\"n\"}\"#)"));
    assert!(text.contains("assert_eq!(got, want, \"output mismatch\");"));
}

#[test]
fn an_open_struct_pattern_checks_fields_and_markers_over_the_wire_form() {
    let module = module_with(vec![outcome_test(
        "matches loosely",
        TestPattern::Struct(struct_pattern(
            true,
            vec![
                ("id", eq(serde_json::json!("c1"))),
                ("created", present()),
                ("gone", absent()),
            ],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("let got = serde_json::to_value(&out).expect(\"encode output\");"));
    assert!(text.contains("assert_eq!(got.get(\"id\"), Some(&want), \"field id\");"));
    assert!(text
        .contains("assert!(got.get(\"created\").is_some(), \"field created must be present\");"));
    assert!(text.contains("assert!(got.get(\"gone\").is_none(), \"field gone must be absent\");"));
    // Open: unmentioned keys pass.
    assert!(!text.contains("unexpected field"));
}

#[test]
fn a_closed_struct_pattern_with_a_marker_rejects_unmentioned_keys() {
    let module = module_with(vec![outcome_test(
        "pins the keys",
        TestPattern::Struct(struct_pattern(
            false,
            vec![("id", eq(serde_json::json!("c1"))), ("created", present())],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("if let Some(object) = got.as_object() {"));
    assert!(text.contains(
        "assert!(matches!(key.as_str(), \"created\" | \"id\"), \"unexpected field {key}\");"
    ));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let module = module_with(vec![
        outcome_test(
            "open error fields",
            TestPattern::Error(error_pattern(
                true,
                vec![
                    ("reason", eq(serde_json::json!("r"))),
                    ("hint", present()),
                    ("gone", absent()),
                ],
            )),
        ),
        outcome_test(
            "closed error total",
            TestPattern::Error(error_pattern(
                false,
                vec![("reason", eq(serde_json::json!("r")))],
            )),
        ),
        outcome_test(
            "bare error",
            TestPattern::Error(error_pattern(true, vec![])),
        ),
    ]);
    let text = hermetic_text(&module);
    // The failure must be the declared typed error, unwrapped in two steps.
    assert!(text.contains("Err(TonoError::Api(failure)) => failure,"));
    assert!(text.contains("APIFailure::PaymentDeclined(data) => data,"));
    assert!(text.contains("panic!(\"want the declared error payment_declined, got {other:?}\"),"));
    // Open with fields: per-field checks over the re-encoded error data.
    assert!(
        text.contains("let got = serde_json::to_value(&declared).expect(\"encode error data\");")
    );
    assert!(text.contains("assert_eq!(got.get(\"reason\"), Some(&want), \"field reason\");"));
    assert!(text.contains("assert!(got.get(\"hint\").is_some(), \"field hint must be present\");"));
    assert!(text.contains("assert!(got.get(\"gone\").is_none(), \"field gone must be absent\");"));
    // Closed with only eq fields: one total comparison of the error data.
    assert!(text.contains("serde_json::from_str(r#\"{\"reason\":\"r\"}\"#)"));
    assert!(text.contains("assert_eq!(got, want, \"error data mismatch\");"));
    // A bare open pattern stops at the typed unwrap.
    assert!(text.contains("let _declared = match failure {"));
}

#[test]
fn taxonomy_patterns_cover_every_category_with_and_without_fields() {
    let tax = |category: &str, fields: Vec<(&str, FieldPattern)>| {
        TestPattern::Taxonomy(TaxonomyPattern {
            category: category.into(),
            open: true,
            fields: fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        })
    };
    let module = module_with(vec![
        outcome_test(
            "api tax",
            tax(
                "api",
                vec![
                    ("status", eq(serde_json::json!(500))),
                    ("body", eq(serde_json::json!("boom"))),
                ],
            ),
        ),
        outcome_test("api bare", tax("api", vec![])),
        outcome_test(
            "validation tax",
            tax(
                "validation",
                vec![("fields", eq(serde_json::json!(["id"])))],
            ),
        ),
        outcome_test("validation bare", tax("validation", vec![])),
        outcome_test(
            "decode tax",
            tax("decode", vec![("path", eq(serde_json::json!("$.id")))]),
        ),
        outcome_test("decode bare", tax("decode", vec![])),
        outcome_test(
            "contract tax",
            tax(
                "contract",
                vec![("name", eq(serde_json::json!("create_charge")))],
            ),
        ),
        outcome_test("contract bare", tax("contract", vec![])),
        outcome_test(
            "config tax",
            tax("config", vec![("field", eq(serde_json::json!("api_key")))]),
        ),
        outcome_test("config bare", tax("config", vec![])),
        outcome_test("transport tax", tax("transport", vec![])),
    ]);
    let text = hermetic_text(&module);
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
    let mut stub = http_stub();
    stub.binding = Some("s".into());
    let module = module_with(vec![TestDecl {
        name: "traces the request".into(),
        constructions: vec![construction()],
        stubs: vec![stub],
        calls: vec![call()],
        expects: vec![TestExpect::Requests {
            subject: "s".into(),
            requests: vec![RequestPattern {
                open: true,
                fields: BTreeMap::new(),
                headers: Some(BTreeMap::from([
                    ("X-Trace".to_string(), present()),
                    ("X-Debug".to_string(), absent()),
                ])),
            }],
        }],
    }]);
    let text = hermetic_text(&module);
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
    assert!(expects::eq_str(&present()).is_none());
    assert!(expects::eq_str(&eq(serde_json::json!(1))).is_none());
    assert_eq!(expects::eq_str(&eq(serde_json::json!("s"))), Some("s"));
    assert!(expects::eq_value(&absent()).is_none());
    assert_eq!(
        expects::eq_value(&eq(serde_json::json!(7))),
        Some(&serde_json::json!(7))
    );
}

#[test]
fn map_field_asserts_skips_a_nested_structural_pattern() {
    let pattern = struct_pattern(
        true,
        vec![(
            "nested",
            FieldPattern::Pat(TestPattern::Struct(struct_pattern(true, vec![]))),
        )],
    );
    assert_eq!(expects::map_field_asserts(&pattern), "");
}

#[test]
fn a_closed_pattern_with_no_fields_rejects_every_key() {
    // Both emitter callers collapse a closed all-eq pattern (and the empty
    // field map is trivially all-eq) before reaching the per-field spelling,
    // so the no-arms rejection is pinned here at the unit seam.
    let text = expects::map_field_asserts(&struct_pattern(false, vec![]));
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
    let module = module_with(vec![outcome_test("base", TestPattern::Ok(Empty {}))]);
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
    let module = module_with(vec![outcome_test(
        "eq outcome",
        TestPattern::Eq(serde_json::json!({"id": "c1"})),
    )]);
    with_ctx(&module, |ctx| {
        let text = expects::outcome_asserts(ctx, false);
        assert_eq!(text, "    result.map(|_| ()).expect(\"want ok\");\n");
    });
}

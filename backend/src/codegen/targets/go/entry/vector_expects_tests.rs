//! Emitter tests for the expectation half of the generated Go tests: every
//! outcome-pattern arm (equality, open/closed struct and error patterns, the
//! taxonomy categories), the request marker headers, and the defensive arms
//! validation keeps out of the pipeline.

use std::collections::BTreeMap;

use super::super::tests::fixture_module;
use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::ops::error_names;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::{push_entry_op_trait, rendered};
use crate::ir::{
    Empty, FieldPattern, HttpAnswer, Module, RequestPattern, ShapePattern, StubAnswer, StubDep,
    TaxonomyPattern, TestCall, TestConstruction, TestDecl, TestExpect, TestPattern, TestStub,
};

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
        binding: "saved".into(),
        client: "c".into(),
        op: "save_note".into(),
        input: Some(serde_json::json!({"id": "n1"})),
    }
}

fn http_stub() -> TestStub {
    TestStub {
        binding: None,
        client: "c".into(),
        op: "save_note".into(),
        dep: StubDep::Http,
        answers: vec![StubAnswer::Http(HttpAnswer {
            status: 200,
            headers: BTreeMap::new(),
            body: "{\"id\":\"n1\"}".into(),
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
            subject: "saved".into(),
            pattern,
        }],
    }
}

fn struct_pattern(open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
    ShapePattern {
        shape: "note".into(),
        open,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

fn error_pattern(open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
    ShapePattern {
        shape: "overloaded".into(),
        open,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    }
}

/// The schema fixture with a wire descriptor pushed onto its operation so an
/// http stub validates, carrying the given declared tests.
fn wired_module(tests: Vec<TestDecl>) -> Module {
    let mut module = fixture_module();
    push_entry_op_trait(
        &mut module,
        "wire_descriptor",
        serde_json::json!({"http_method": "POST", "uri": "/notes", "bindings": {}}),
    );
    module.tests = tests;
    module
}

fn hermetic_text(module: &Module) -> String {
    let files = super::test_files(module, &go_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &GoRules::default())
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let module = wired_module(vec![outcome_test("just works", TestPattern::Ok(Empty {}))]);
    let text = hermetic_text(&module);
    assert!(text.contains("t.Fatalf(\"want ok, got error: %v\", err)"));
    assert!(!text.contains("json.Marshal(out)"));
}

#[test]
fn a_closed_all_eq_struct_pattern_collapses_into_one_total_comparison() {
    let module = wired_module(vec![outcome_test(
        "pins the wire object",
        TestPattern::Struct(struct_pattern(
            false,
            vec![
                ("id", eq(serde_json::json!("n1"))),
                ("body", eq(serde_json::json!("b"))),
            ],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("blob, err := json.Marshal(out)"));
    assert!(text.contains("var got any"));
    assert!(text.contains("json.Unmarshal([]byte(`{\"body\":\"b\",\"id\":\"n1\"}`), &want)"));
    assert!(text.contains("if !reflect.DeepEqual(got, want) {"));
    assert!(!text.contains("map[string]any"));
}

#[test]
fn an_open_struct_pattern_checks_fields_and_markers_over_the_wire_form() {
    let module = wired_module(vec![outcome_test(
        "matches loosely",
        TestPattern::Struct(struct_pattern(
            true,
            vec![
                ("id", eq(serde_json::json!("n1"))),
                ("body", present()),
                ("extra", absent()),
            ],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("var got map[string]any"));
    assert!(text.contains("if !reflect.DeepEqual(got[\"id\"], want) {"));
    assert!(text.contains("if _, ok := got[\"body\"]; !ok {"));
    assert!(text.contains("t.Errorf(\"field body must be present\")"));
    assert!(text.contains("if _, ok := got[\"extra\"]; ok {"));
    assert!(text.contains("t.Errorf(\"field extra must be absent\")"));
    // Open: unmentioned keys pass.
    assert!(!text.contains("unexpected field"));
}

#[test]
fn a_closed_struct_pattern_with_a_marker_rejects_unmentioned_keys() {
    let module = wired_module(vec![outcome_test(
        "pins the keys",
        TestPattern::Struct(struct_pattern(
            false,
            vec![("id", eq(serde_json::json!("n1"))), ("body", present())],
        )),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("for key := range got {"));
    assert!(text.contains("case \"body\", \"id\":"));
    assert!(text.contains("t.Errorf(\"unexpected field %q\", key)"));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let module = wired_module(vec![
        outcome_test(
            "open error fields",
            TestPattern::Error(error_pattern(
                true,
                vec![("message", eq(serde_json::json!("busy")))],
            )),
        ),
        outcome_test(
            "closed error total",
            TestPattern::Error(error_pattern(
                false,
                vec![("message", eq(serde_json::json!("busy")))],
            )),
        ),
    ]);
    let text = hermetic_text(&module);
    // The failure must be the declared typed error.
    assert!(text.contains("var declared *Overloaded"));
    assert!(text.contains("if !errors.As(err, &declared) {"));
    assert!(text.contains("t.Fatalf(\"want the declared error overloaded, got %v\", err)"));
    // Open with fields: the error data decodes into a wire map for the
    // per-field checks.
    assert!(text.contains("blob, err := json.Marshal(declared)"));
    assert!(text.contains("t.Fatalf(\"decode encoded error data: %v\", err)"));
    // Closed with only eq fields: one total comparison of the error data.
    assert!(text.contains("t.Fatalf(\"encode error data: %v\", err)"));
    assert!(text.contains("json.Unmarshal([]byte(`{\"message\":\"busy\"}`), &want)"));
}

#[test]
fn a_bare_open_error_pattern_stops_at_the_typed_check() {
    let module = wired_module(vec![outcome_test(
        "bare error",
        TestPattern::Error(error_pattern(true, vec![])),
    )]);
    let text = hermetic_text(&module);
    assert!(text.contains("if !errors.As(err, &declared) {"));
    assert!(!text.contains("error data"));
}

#[test]
fn taxonomy_patterns_cover_every_category() {
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
    let module = wired_module(vec![
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
        outcome_test(
            "validation tax",
            tax(
                "validation",
                vec![("fields", eq(serde_json::json!(["id"])))],
            ),
        ),
        outcome_test(
            "decode tax",
            tax("decode", vec![("path", eq(serde_json::json!("$.id")))]),
        ),
        outcome_test(
            "contract tax",
            tax(
                "contract",
                vec![("name", eq(serde_json::json!("save_note")))],
            ),
        ),
        outcome_test(
            "config tax",
            tax("config", vec![("field", eq(serde_json::json!("api_key")))]),
        ),
        outcome_test("transport tax", tax("transport", vec![])),
    ]);
    let text = hermetic_text(&module);
    assert!(text.contains("var api *APIError"));
    assert!(text.contains("if api.Status != 500 {"));
    assert!(text.contains("if api.Body != `boom` {"));
    assert!(text.contains("var invalid *ValidationError"));
    assert!(text.contains("violated := []string{}"));
    assert!(text.contains("if want := []string{\"id\", }; !reflect.DeepEqual(violated, want) {"));
    assert!(text.contains("var bad *DecodeError"));
    assert!(text.contains("if bad.Path != \"$.id\" {"));
    assert!(text.contains("var broken *ContractError"));
    assert!(text.contains("if broken.ContractName != \"save_note\" {"));
    assert!(text.contains("var cfg *ConfigError"));
    assert!(text.contains("if !strings.Contains(cfg.Message, \"api_key\") {"));
    assert!(text.contains("var down *TransportError"));
}

#[test]
fn request_header_markers_check_presence_and_absence() {
    let mut stub = http_stub();
    stub.binding = Some("s".into());
    let module = wired_module(vec![TestDecl {
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
    assert!(text.contains("if len(seen) != 1 {"));
    assert!(text.contains("lower0 := map[string]string{}"));
    assert!(text.contains("if _, ok := lower0[\"x-trace\"]; !ok {"));
    assert!(text.contains("t.Errorf(\"request 0 header X-Trace must be present\")"));
    assert!(text.contains("if _, ok := lower0[\"x-debug\"]; ok {"));
    assert!(text.contains("t.Errorf(\"request 0 header X-Debug must be absent\")"));
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
    let mut refs = Vec::new();
    let pattern = struct_pattern(
        true,
        vec![(
            "nested",
            FieldPattern::Pat(TestPattern::Struct(struct_pattern(true, vec![]))),
        )],
    );
    assert_eq!(expects::map_field_asserts(&pattern, &mut refs), "");
}

#[test]
fn a_closed_pattern_with_no_fields_rejects_every_key() {
    // Both emitter callers collapse a closed all-eq pattern (and the empty
    // field map is trivially all-eq) before reaching the per-field spelling,
    // so the no-arms switch is pinned here at the unit seam.
    let mut refs = Vec::new();
    let text = expects::map_field_asserts(&struct_pattern(false, vec![]), &mut refs);
    assert!(text.contains("for key := range got {"));
    assert!(!text.contains("case "));
    assert!(text.contains("t.Errorf(\"unexpected field %q\", key)"));
}

#[test]
fn an_unknown_taxonomy_category_fails_loudly_in_the_generated_test() {
    let mut refs = Vec::new();
    let pattern = TaxonomyPattern {
        category: "bogus".into(),
        open: true,
        fields: BTreeMap::new(),
    };
    let text = expects::taxonomy_asserts(&pattern, &error_names(), &mut refs);
    assert!(text.contains("t.Fatalf(\"unknown error category bogus\")"));
}

#[test]
fn an_unknown_error_shape_fails_loudly_in_the_generated_test() {
    let module = wired_module(vec![outcome_test("base", TestPattern::Ok(Empty {}))]);
    let (entries, multi, _bound) =
        plan::entry_setup(&module, &super::BINDING_LANGS).expect("the fixture has an entry");
    let planned = declared_tests::entry_tests(&module).expect("the declared tests validate");
    let entry = &entries[0];
    let n = super::super::names(entry, multi);
    let config = go_casing();
    let ctx = super::TestCtx {
        entry,
        n: &n,
        module: &module,
        config: &config,
        multi,
        test: &planned[0].tests[0],
    };
    let mut refs = Vec::new();
    let text = expects::error_asserts(
        &ctx,
        &ShapePattern {
            shape: "nope".into(),
            open: true,
            fields: BTreeMap::new(),
        },
        &mut refs,
    );
    assert!(text.contains("t.Fatalf(\"pattern names unknown error shape nope\")"));
}

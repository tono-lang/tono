//! Emitter tests for the expectation half of the generated Vitest tests:
//! every outcome-pattern arm (equality, open/closed struct and error
//! patterns, the taxonomy categories), the request marker headers, and the
//! defensive arms validation refuses to let through the pipeline.

use std::collections::BTreeMap;

use super::super::tests::fixture_module;
use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::{push_entry_op_trait, rendered, set_entry_op_outputs};
use crate::ir::{
    Empty, FieldPattern, HttpAnswer, Module, Prim, RequestPattern, ShapePattern, StubAnswer,
    StubDep, TaxonomyPattern, TestCall, TestConstruction, TestDecl, TestExpect, TestPattern,
    TestStub, Tref,
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
    let files = super::test_files(module, &ts_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &TsRules)
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let module = wired_module(vec![outcome_test("just works", TestPattern::Ok(Empty {}))]);
    let text = hermetic_text(&module);
    assert!(text.contains("await c.saveNote(input);"));
    assert!(!text.contains("const out"));
    assert!(!text.contains("expect(out"));
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
    // The wire form is what the per-field checks read.
    assert!(text.contains("const got = encodeNote(out) as Record<string, unknown>;"));
    assert!(text.contains("expect(got[\"id\"]).toEqual(\"n1\");"));
    assert!(text.contains("expect(got[\"body\"]).not.toBeUndefined();"));
    assert!(text.contains("expect(got[\"extra\"]).toBeUndefined();"));
    // Open: unmentioned keys pass.
    assert!(!text.contains("Object.entries(got)"));
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
    assert!(text.contains("expect(encodeNote(out)).toEqual({\"body\":\"b\",\"id\":\"n1\"});"));
    assert!(!text.contains("const got"));
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
    assert!(text.contains("Object.entries(got)"));
    assert!(text.contains("![\"body\", \"id\"].includes(k)"));
    assert!(text.contains(").toEqual([]);"));
}

#[test]
fn a_primitive_output_compares_the_value_itself_without_a_codec() {
    let mut module = wired_module(vec![
        outcome_test("eq prim", TestPattern::Eq(serde_json::json!("x"))),
        outcome_test(
            "struct prim",
            TestPattern::Struct(struct_pattern(
                true,
                vec![("id", eq(serde_json::json!("n1")))],
            )),
        ),
    ]);
    set_entry_op_outputs(&mut module, Tref::Prim(Prim::String));
    let text = hermetic_text(&module);
    assert!(text.contains("expect(out).toEqual(\"x\");"));
    assert!(text.contains("const got = out as Record<string, unknown>;"));
    assert!(!text.contains("encodeNote(out)"));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let module = wired_module(vec![
        outcome_test(
            "open error fields",
            TestPattern::Error(error_pattern(
                true,
                vec![
                    ("message", eq(serde_json::json!("busy"))),
                    ("bogus", eq(serde_json::json!("x"))),
                ],
            )),
        ),
        outcome_test(
            "open error present",
            TestPattern::Error(error_pattern(true, vec![("message", present())])),
        ),
        outcome_test(
            "open error absent",
            TestPattern::Error(error_pattern(true, vec![("message", absent())])),
        ),
        outcome_test(
            "closed error marker",
            TestPattern::Error(error_pattern(false, vec![("message", present())])),
        ),
        outcome_test(
            "closed error total",
            TestPattern::Error(error_pattern(
                false,
                vec![("message", eq(serde_json::json!("busy")))],
            )),
        ),
        outcome_test(
            "bare error",
            TestPattern::Error(error_pattern(true, vec![])),
        ),
    ]);
    let text = hermetic_text(&module);
    // The failure lands as the declared typed error, caught off the call.
    assert!(text.contains("let caught: unknown;"));
    assert!(text.contains("expect(caught).toBeInstanceOf(OverloadedError);"));
    assert!(text.contains("const declared = caught as OverloadedError;"));
    // A field equality re-encodes the decoded member for the wire comparison.
    assert!(text.contains("expect(declared.data.message).toEqual(\"busy\");"));
    // A pattern field naming no member of the shape is skipped.
    assert!(!text.contains("bogus"));
    // Markers check presence and absence on the decoded data.
    assert!(text.contains("expect(declared.data.message).not.toBeUndefined();"));
    assert!(text.contains("expect(declared.data.message).toBeUndefined();"));
    // A closed pattern with a marker rejects unmentioned keys of the data.
    assert!(text.contains("Object.entries(declared.data as unknown as Record<string, unknown>)"));
    assert!(text.contains("![\"message\"].includes(k)"));
    // Closed with only eq fields: one total comparison of the re-encoded data.
    assert!(
        text.contains("expect(encodeOverloaded(declared.data)).toEqual({\"message\":\"busy\"});")
    );
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
    assert!(text.contains("expect(caught).toBeInstanceOf(APIError);"));
    assert!(text.contains("const api = caught as APIError;"));
    assert!(text.contains("expect(api.status).toBe(500);"));
    assert!(text.contains("expect(api.body).toBe(\"boom\");"));
    assert!(text.contains("expect(caught).toBeInstanceOf(ValidationError);"));
    assert!(text.contains(".violations.map((x) => x.field)).toEqual([\"id\"]);"));
    assert!(text.contains("expect((caught as DecodeError).path).toBe(\"$.id\");"));
    assert!(text.contains("expect((caught as ContractError).contractName).toBe(\"save_note\");"));
    assert!(text.contains("expect((caught as ConfigError).message).toContain(\"api_key\");"));
    assert!(text.contains("expect(caught).toBeInstanceOf(TransportError);"));
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
    assert!(text.contains("expect(seen.length).toBe(1);"));
    assert!(text.contains("const lower0 = Object.fromEntries("));
    assert!(text.contains("expect(lower0[\"x-trace\"]).not.toBeUndefined();"));
    assert!(text.contains("expect(lower0[\"x-debug\"]).toBeUndefined();"));
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
    assert_eq!(expects::map_field_asserts("got", &pattern), "");
}

/// A `TestCtx` over the wired fixture's first (valid) planned test, for the
/// defensive arms the shared validation keeps out of the pipeline.
fn with_ctx(module: &Module, f: impl FnOnce(&super::TestCtx<'_>)) {
    let (entries, multi, bound) =
        plan::entry_setup(module, &super::BINDING_LANGS).expect("the fixture has an entry");
    let planned = declared_tests::entry_tests(module).expect("the declared tests validate");
    let entry = &entries[0];
    let n = super::super::names(entry, multi);
    let config = ts_casing();
    let ctx = super::TestCtx {
        entry,
        n: &n,
        module,
        config: &config,
        bound: &bound,
        test: &planned[0].tests[0],
    };
    f(&ctx);
}

#[test]
fn the_defensive_arms_fail_loudly_in_the_generated_test() {
    let module = wired_module(vec![outcome_test("base", TestPattern::Ok(Empty {}))]);
    with_ctx(&module, |ctx| {
        let mut refs = Vec::new();
        // A success pattern never reaches the failure asserts.
        assert_eq!(
            expects::failure_asserts(ctx, &TestPattern::Ok(Empty {}), &mut refs),
            ""
        );
        // An unknown error shape generates a throwing test, not a panic.
        let unknown = TestPattern::Error(ShapePattern {
            shape: "nope".into(),
            open: true,
            fields: BTreeMap::new(),
        });
        let text = expects::failure_asserts(ctx, &unknown, &mut refs);
        assert!(text.contains("throw new Error(\"pattern names unknown error shape nope\");"));
        // An unknown taxonomy category likewise.
        let tax = TaxonomyPattern {
            category: "bogus".into(),
            open: true,
            fields: BTreeMap::new(),
        };
        let text = expects::taxonomy_asserts(ctx, &tax, &mut refs);
        assert!(text.contains("throw new Error(\"unknown error category bogus\");"));
        // A nested structural pattern in error data is skipped.
        let nested = error_pattern(
            true,
            vec![(
                "message",
                FieldPattern::Pat(TestPattern::Struct(struct_pattern(true, vec![]))),
            )],
        );
        assert_eq!(
            expects::error_data_asserts(ctx, "notes#overloaded", &nested, &mut refs),
            ""
        );
        // A shape id resolving to a non-structure shape carries no members,
        // so every pattern field is skipped.
        assert_eq!(
            expects::error_data_asserts(
                ctx,
                "notes#conf",
                &error_pattern(true, vec![("message", present())]),
                &mut refs,
            ),
            ""
        );
    });
}

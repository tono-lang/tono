//! Emitter tests for the expectation half of the generated Vitest tests:
//! every outcome-pattern arm (equality, open/closed struct and error
//! patterns, the taxonomy categories), the request marker headers, and the
//! defensive arms validation refuses to let through the pipeline. The
//! declared tests come from the shared bed; only the assertions over the
//! generated TypeScript live here.

use std::collections::BTreeMap;

use super::super::tests::fixture_module;
use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::{
    absent, eq, notes_bed, present, rendered, set_entry_op_outputs, wired,
};
use crate::ir::{
    Empty, FieldPattern, Module, Prim, ShapePattern, TaxonomyPattern, TestPattern, Tref,
};

fn hermetic_text(module: &Module) -> String {
    let files = super::test_files(module, &ts_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &TsRules)
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let text = hermetic_text(&wired(fixture_module(), vec![notes_bed().ok_test()]));
    assert!(text.contains("await c.saveNote(input);"));
    assert!(!text.contains("const out"));
    assert!(!text.contains("expect(out"));
}

#[test]
fn an_open_struct_pattern_checks_fields_and_markers_over_the_wire_form() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().open_struct_test()],
    ));
    // The wire form is what the per-field checks read.
    assert!(text.contains("const got = encodeNote(out) as Record<string, unknown>;"));
    assert!(text.contains("expect(got[\"id\"]).toEqual(\"n1\");"));
    assert!(text.contains("expect(got[\"extra\"]).not.toBeUndefined();"));
    assert!(text.contains("expect(got[\"missing\"]).toBeUndefined();"));
    // Open: unmentioned keys pass.
    assert!(!text.contains("Object.entries(got)"));
}

#[test]
fn a_closed_all_eq_struct_pattern_collapses_into_one_total_comparison() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().closed_eq_struct_test()],
    ));
    assert!(text.contains("expect(encodeNote(out)).toEqual({\"id\":\"n1\",\"tag\":\"t\"});"));
    assert!(!text.contains("const got"));
}

#[test]
fn a_closed_struct_pattern_with_a_marker_rejects_unmentioned_keys() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().closed_marker_struct_test()],
    ));
    assert!(text.contains("Object.entries(got)"));
    assert!(text.contains("![\"id\", \"tag\"].includes(k)"));
    assert!(text.contains(").toEqual([]);"));
}

#[test]
fn a_primitive_output_compares_the_value_itself_without_a_codec() {
    let bed = notes_bed();
    let mut module = wired(
        fixture_module(),
        vec![
            bed.outcome_test("eq prim", TestPattern::Eq(serde_json::json!("x"))),
            bed.outcome_test(
                "struct prim",
                TestPattern::Struct(
                    bed.struct_pattern(true, vec![("id", eq(bed.input["id"].clone()))]),
                ),
            ),
        ],
    );
    set_entry_op_outputs(&mut module, Tref::Prim(Prim::String));
    let text = hermetic_text(&module);
    assert!(text.contains("expect(out).toEqual(\"x\");"));
    assert!(text.contains("const got = out as Record<string, unknown>;"));
    assert!(!text.contains("encodeNote(out)"));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let text = hermetic_text(&wired(fixture_module(), notes_bed().error_suite()));
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
    let text = hermetic_text(&wired(fixture_module(), notes_bed().taxonomy_suite()));
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
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().request_marker_test()],
    ));
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
    let bed = notes_bed();
    let nested = FieldPattern::Pat(TestPattern::Struct(bed.struct_pattern(true, vec![])));
    let pattern = bed.struct_pattern(true, vec![("nested", nested)]);
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
    let bed = notes_bed();
    let module = wired(fixture_module(), vec![bed.ok_test()]);
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
        let nested = bed.error_pattern(
            true,
            vec![(
                "message",
                FieldPattern::Pat(TestPattern::Struct(bed.struct_pattern(true, vec![]))),
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
                &bed.error_pattern(true, vec![("message", present())]),
                &mut refs,
            ),
            ""
        );
    });
}

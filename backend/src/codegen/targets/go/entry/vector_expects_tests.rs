//! Emitter tests for the expectation half of the generated Go tests: every
//! outcome-pattern arm (equality, open/closed struct and error patterns, the
//! taxonomy categories), the request marker headers, and the defensive arms
//! validation keeps out of the pipeline. The declared tests come from the
//! shared bed; only the assertions over the generated Go live here.

use std::collections::BTreeMap;

use super::super::tests::fixture_module;
use super::expects;
use crate::codegen::declared_tests;
use crate::codegen::entries::plan;
use crate::codegen::ops::error_names;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::{absent, eq, notes_bed, present, rendered, wired};
use crate::ir::{FieldPattern, Module, ShapePattern, TaxonomyPattern, TestPattern};

fn hermetic_text(module: &Module) -> String {
    let files = super::test_files(module, &go_casing());
    assert!(!files.is_empty(), "the declared tests generate a file");
    rendered(&files[0].file.decls, &GoRules::default())
}

#[test]
fn an_ok_pattern_asserts_nothing_beyond_the_successful_call() {
    let text = hermetic_text(&wired(fixture_module(), vec![notes_bed().ok_test()]));
    assert!(text.contains("t.Fatalf(\"want ok, got error: %v\", err)"));
    assert!(!text.contains("json.Marshal(out)"));
}

#[test]
fn a_closed_all_eq_struct_pattern_collapses_into_one_total_comparison() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().closed_eq_struct_test()],
    ));
    assert!(text.contains("blob, err := json.Marshal(out)"));
    assert!(text.contains("var got any"));
    assert!(text.contains("json.Unmarshal([]byte(`{\"id\":\"n1\",\"tag\":\"t\"}`), &want)"));
    assert!(text.contains("if !reflect.DeepEqual(got, want) {"));
    assert!(!text.contains("map[string]any"));
}

#[test]
fn an_open_struct_pattern_checks_fields_and_markers_over_the_wire_form() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().open_struct_test()],
    ));
    assert!(text.contains("var got map[string]any"));
    assert!(text.contains("if !reflect.DeepEqual(got[\"id\"], want) {"));
    assert!(text.contains("if _, ok := got[\"extra\"]; !ok {"));
    assert!(text.contains("t.Errorf(\"field extra must be present\")"));
    assert!(text.contains("if _, ok := got[\"missing\"]; ok {"));
    assert!(text.contains("t.Errorf(\"field missing must be absent\")"));
    // Open: unmentioned keys pass.
    assert!(!text.contains("unexpected field"));
}

#[test]
fn a_closed_struct_pattern_with_a_marker_rejects_unmentioned_keys() {
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().closed_marker_struct_test()],
    ));
    assert!(text.contains("for key := range got {"));
    assert!(text.contains("case \"id\", \"tag\":"));
    assert!(text.contains("t.Errorf(\"unexpected field %q\", key)"));
}

#[test]
fn error_patterns_check_the_declared_error_and_its_data() {
    let text = hermetic_text(&wired(fixture_module(), notes_bed().error_suite()));
    // The failure must be the declared typed error.
    assert!(text.contains("var declared *Overloaded"));
    assert!(text.contains("if !errors.As(err, &declared) {"));
    assert!(text.contains("t.Fatalf(\"want the declared error overloaded, got %v\", err)"));
    // Open with fields: the error data decodes into a wire map for the
    // per-field checks, markers included.
    assert!(text.contains("blob, err := json.Marshal(declared)"));
    assert!(text.contains("t.Fatalf(\"decode encoded error data: %v\", err)"));
    assert!(text.contains("if _, ok := got[\"message\"]; !ok {"));
    assert!(text.contains("if _, ok := got[\"message\"]; ok {"));
    // Closed with only eq fields: one total comparison of the error data.
    assert!(text.contains("t.Fatalf(\"encode error data: %v\", err)"));
    assert!(text.contains("json.Unmarshal([]byte(`{\"message\":\"busy\"}`), &want)"));
}

#[test]
fn a_bare_open_error_pattern_stops_at_the_typed_check() {
    let bed = notes_bed();
    let bare = bed.outcome_test(
        "bare error",
        TestPattern::Error(bed.error_pattern(true, vec![])),
    );
    let text = hermetic_text(&wired(fixture_module(), vec![bare]));
    assert!(text.contains("if !errors.As(err, &declared) {"));
    assert!(!text.contains("error data"));
}

#[test]
fn taxonomy_patterns_cover_every_category() {
    let text = hermetic_text(&wired(fixture_module(), notes_bed().taxonomy_suite()));
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
    let text = hermetic_text(&wired(
        fixture_module(),
        vec![notes_bed().request_marker_test()],
    ));
    assert!(text.contains("if len(seen) != 1 {"));
    assert!(text.contains("lower0 := map[string]string{}"));
    assert!(text.contains("if _, ok := lower0[\"x-trace\"]; !ok {"));
    assert!(text.contains("t.Errorf(\"request 0 header X-Trace must be present\")"));
    assert!(text.contains("if _, ok := lower0[\"x-debug\"]; ok {"));
    assert!(text.contains("t.Errorf(\"request 0 header X-Debug must be absent\")"));
}

#[test]
fn the_field_leaf_readers_accept_equality_only() {
    assert_eq!(expects::eq_str(&eq(serde_json::json!("s"))), Some("s"));
    for other in [present(), absent(), eq(serde_json::json!(1))] {
        assert!(expects::eq_str(&other).is_none());
    }
    assert!(expects::eq_value(&present()).is_none());
    assert_eq!(
        expects::eq_value(&eq(serde_json::json!(7))),
        Some(&serde_json::json!(7))
    );
}

#[test]
fn map_field_asserts_skips_a_nested_structural_pattern() {
    let bed = notes_bed();
    let mut refs = Vec::new();
    let nested = FieldPattern::Pat(TestPattern::Struct(bed.struct_pattern(true, vec![])));
    let pattern = bed.struct_pattern(true, vec![("nested", nested)]);
    assert_eq!(expects::map_field_asserts(&pattern, &mut refs), "");
}

#[test]
fn a_closed_pattern_with_no_fields_rejects_every_key() {
    // Both emitter callers collapse a closed all-eq pattern (and the empty
    // field map is trivially all-eq) before reaching the per-field spelling,
    // so the no-arms switch is pinned here at the unit seam.
    let mut refs = Vec::new();
    let text = expects::map_field_asserts(&notes_bed().struct_pattern(false, vec![]), &mut refs);
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
    let module = wired(fixture_module(), vec![notes_bed().ok_test()]);
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

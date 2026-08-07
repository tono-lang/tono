//! Unit tests for the pure per-operation builders in `transport.rs`, split
//! from it to stay within the file-size gate. `entry/tests.rs` and the
//! codegen snapshot cover the assembled method end to end; these pin the
//! individual branches (label vs query vs payload, response bindings, the
//! all-body fast path) that a fixture-level test would only exercise one of
//! at a time.

use super::*;
use crate::codegen::entries::wire::needs_record;
use crate::ir::{TemplatePart, WireBinding, WireResponsePart, WireValue};

fn wire() -> WireBinding {
    WireBinding {
        method: "GET".into(),
        uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
        body: None,
        response_bindings: Default::default(),
        success: Vec::new(),
        endpoint: None,
        request_headers: Vec::new(),
        query: vec![],
        timeout: None,
        retry: None,
    }
}

/// A stub standing in for the real `this.settings.<path>` closure `op_method`
/// builds from the entry model: exercises the same call shape without
/// needing a real `EntryModel` fixture in these pure-function tests.
fn stub_field_expr(path: &[String]) -> String {
    format!("this.settings.{}", path.join("."))
}

/// A stub standing in for the real param-member resolver `op_method` builds
/// off the op's input type: always unresolved, matching every case in this
/// file (the resolved path is pinned separately, see the tests below).
fn stub_param_access(_: &str) -> Option<String> {
    None
}

// The range-vs-exact-match logic itself is proven once, target-agnostically,
// by `success_test_expr`'s own tests in `codegen::entries::wire`; this only
// pins the TypeScript field/operator wiring (`response.status`, `===`).
#[test]
fn success_expr_spells_the_typescript_field_and_operator() {
    let mut w = wire();
    w.success = vec![200, 404];
    assert_eq!(
        success_expr(&w),
        "response.status === 200 || response.status === 404"
    );
}

#[test]
fn uri_expr_renders_a_pure_literal_without_a_template_wrapper() {
    let w = wire();
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "\"/x\""
    );
}

#[test]
fn uri_expr_mixes_literal_field_and_input_placeholders() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/notes/".into()),
        TemplatePart::Input("id".into()),
        TemplatePart::Lit("/".into()),
        TemplatePart::Field(vec!["region".into()]),
    ]);
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "`/notes/${pathPart(record[\"id\"])}/${pathPart(this.settings.region)}`"
    );
}

// ── Named op-parameter references (WireValue::Param / TemplatePart::Param) ─

#[test]
fn a_bare_param_reference_in_a_uri_template_reads_the_whole_input() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec![]),
    ]);
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "`/charges/${pathPart(input)}`"
    );
}

#[test]
fn a_param_member_reference_in_a_uri_template_reads_off_the_record() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "`/charges/${pathPart(record[\"id\"])}`"
    );
}

// ── A resolvable param member reads straight off the typed input, and the
//    record disappears entirely when nothing else in the operation needs it.

fn resolving(field: &'static str) -> impl Fn(&str) -> Option<String> {
    move |name| (name == "id").then(|| format!("input.{field}"))
}

#[test]
fn a_resolved_param_member_in_a_uri_template_reads_the_typed_property() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    let param_access = resolving("chargeId");
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &param_access),
        "`/charges/${pathPart(input.chargeId)}`"
    );
}

#[test]
fn a_resolved_param_member_in_a_header_value_reads_the_typed_property() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec!["id".into()]),
    )];
    let param_access = resolving("chargeId");
    let out = declared_header_lines(&w, "  ", &stub_field_expr, "input", &param_access);
    assert!(out.contains("formatScalar(input.chargeId)"));
    assert!(!out.contains("record"));
}

#[test]
fn a_resolved_param_member_in_a_query_value_reads_the_typed_property() {
    let mut w = wire();
    w.query = vec![(
        vec![TemplatePart::Lit("id".into())],
        WireValue::Param(vec!["id".into()]),
    )];
    let param_access = resolving("chargeId");
    assert_eq!(
        query_lines(&w, "  ", &stub_field_expr, "input", &param_access),
        "  appendQuery(qs, \"id\", input.chargeId);\n"
    );
}

#[test]
fn a_pure_param_reference_in_uri_position_passes_through_unescaped() {
    let mut w = wire();
    w.uri = WireValue::Param(vec![]);
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "formatScalar(input)"
    );
}

#[test]
fn a_pure_param_member_reference_in_uri_position_passes_through_unescaped() {
    let mut w = wire();
    w.uri = WireValue::Param(vec!["href".into()]);
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "formatScalar(record[\"href\"])"
    );
}

#[test]
fn a_literal_uri_is_a_plain_string() {
    let mut w = wire();
    w.uri = WireValue::Lit(serde_json::json!("/fixed"));
    assert_eq!(
        uri_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "\"/fixed\""
    );
}

#[test]
fn a_field_endpoint_keeps_its_original_unwrapped_spelling() {
    let mut w = wire();
    w.endpoint = Some(WireValue::Field(vec!["endpoint".into()]));
    assert_eq!(
        endpoint_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "this.settings.endpoint"
    );
}

#[test]
fn a_param_endpoint_goes_through_the_general_renderer() {
    let mut w = wire();
    w.endpoint = Some(WireValue::Param(vec![]));
    assert_eq!(
        endpoint_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "formatScalar(input)"
    );
}

#[test]
fn a_param_reference_in_a_header_value_reads_the_whole_input() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec![]),
    )];
    let out = declared_header_lines(&w, "  ", &stub_field_expr, "input", &stub_param_access);
    assert!(out.contains("formatScalar(input)"));
}

#[test]
fn a_param_reference_in_a_header_key_reads_off_the_record() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Param(vec!["region".into()])],
        WireValue::Lit(serde_json::json!("v")),
    )];
    let out = declared_header_lines(&w, "  ", &stub_field_expr, "input", &stub_param_access);
    assert!(out.contains("record[\"region\"]"));
}

#[test]
fn body_expr_is_none_with_no_body_declared() {
    let w = wire();
    assert_eq!(
        body_expr(&w, &stub_field_expr, "input", &stub_param_access),
        None
    );
}

#[test]
fn body_expr_builds_the_ctor_mapper_object_when_body_is_an_object() {
    let mut w = wire();
    w.body = Some(WireValue::Object(vec![
        (
            "amount".to_string(),
            WireValue::Param(vec!["amount".into()]),
        ),
        ("note".to_string(), WireValue::Param(vec!["note".into()])),
    ]));
    assert_eq!(
        body_expr(&w, &stub_field_expr, "input", &stub_param_access).as_deref(),
        Some("JSON.stringify({ \"amount\": record[\"amount\"], \"note\": record[\"note\"] })")
    );
}

#[test]
fn body_expr_serializes_the_encoded_input_directly_for_the_whole_param() {
    let mut w = wire();
    w.body = Some(WireValue::Param(vec![]));
    // The whole-parameter form stringifies the input expression directly
    // (correct even under a `@wire` rename); needs_record agrees this case
    // needs no `record` alias, so the input expression is used as-is.
    assert_eq!(
        body_expr(
            &w,
            &stub_field_expr,
            "encodeThing(input)",
            &stub_param_access
        )
        .as_deref(),
        Some("JSON.stringify(encodeThing(input))")
    );
}

#[test]
fn body_expr_reads_one_member_raw_for_a_param_member_body() {
    let mut w = wire();
    w.body = Some(WireValue::Param(vec!["envelope".into()]));
    assert_eq!(
        body_expr(&w, &stub_field_expr, "input", &stub_param_access).as_deref(),
        Some("JSON.stringify(record[\"envelope\"])")
    );
}

#[test]
fn needs_record_is_false_with_no_body_declared() {
    assert!(!needs_record(&wire()));
}

#[test]
fn needs_record_is_false_for_a_whole_param_body() {
    let mut w = wire();
    w.body = Some(WireValue::Param(vec![]));
    assert!(!needs_record(&w));
}

#[test]
fn needs_record_is_true_when_a_uri_input_mixes_with_a_param_member_body() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![TemplatePart::Input("id".into())]);
    w.body = Some(WireValue::Param(vec!["amount".into()]));
    assert!(needs_record(&w));
}

#[test]
fn needs_record_is_true_for_a_declared_query_alone() {
    let mut w = wire();
    w.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tag".into()]),
    )];
    assert!(needs_record(&w));
}

#[test]
fn has_query_and_query_lines_agree_on_declared_query_entries() {
    let mut w = wire();
    assert!(!has_query(&w));
    assert_eq!(
        query_lines(&w, "  ", &stub_field_expr, "input", &stub_param_access),
        ""
    );
    w.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tag".into()]),
    )];
    assert!(has_query(&w));
    assert_eq!(
        query_lines(&w, "  ", &stub_field_expr, "input", &stub_param_access),
        "  appendQuery(qs, \"tag\", record[\"tag\"]);\n"
    );
}

#[test]
#[should_panic(expected = "validate_entries rejects an entry @http op with no endpoint")]
fn endpoint_expr_with_no_declared_endpoint_is_an_emission_defect() {
    endpoint_expr(&wire(), &stub_field_expr, "input", &stub_param_access);
}

#[test]
fn endpoint_expr_reads_the_typed_settings_field_with_no_runtime_guard() {
    let mut w = wire();
    w.endpoint = Some(WireValue::Field(vec!["endpoint".into()]));
    // The frontend guarantees `endpoint:` names a string field, so this
    // needs no `typeof`/`as string` guard and no fallback ternary: the bare
    // typed field access is the whole expression.
    assert_eq!(
        endpoint_expr(&w, &stub_field_expr, "input", &stub_param_access),
        "this.settings.endpoint"
    );
}

#[test]
fn outcome_body_expr_is_the_bare_response_body_with_no_response_bindings() {
    assert_eq!(outcome_body_expr(&wire()), "response.body");
}

#[test]
fn outcome_body_expr_folds_only_on_the_success_path() {
    let mut w = wire();
    w.response_bindings = [
        (
            "trace".to_string(),
            WireResponsePart::Header {
                name: "X-Trace-Id".into(),
            },
        ),
        ("status".to_string(), WireResponsePart::StatusCode),
    ]
    .into_iter()
    .collect();
    let out = outcome_body_expr(&w);
    assert!(out.starts_with("(response.status >= 200 && response.status < 300) ? "));
    assert!(out.ends_with(" : response.body"));
    assert!(out.contains("obj[\"trace\"] = response.headers[\"x-trace-id\"] ?? null;"));
    assert!(out.contains("obj[\"status\"] = response.status;"));
}

#[test]
fn declared_header_lines_render_a_literal_key_with_a_field_value() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Client".into())],
        WireValue::Field(vec!["client_name".into()]),
    )];
    assert_eq!(
        declared_header_lines(&w, "  ", &stub_field_expr, "input", &stub_param_access),
        "  setHeader(headers, \"X-Client\", formatScalar(this.settings.client_name));\n"
    );
}

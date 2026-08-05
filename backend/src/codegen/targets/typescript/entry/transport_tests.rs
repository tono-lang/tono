//! Unit tests for the pure per-operation builders in `transport.rs`, split
//! from it to stay within the file-size gate. `entry/tests.rs` and the
//! codegen snapshot cover the assembled method end to end; these pin the
//! individual branches (label vs query vs payload, response bindings, the
//! all-body fast path) that a fixture-level test would only exercise one of
//! at a time.

use super::*;
use crate::ir::{TemplatePart, WireBinding, WirePart, WireResponsePart, WireValue};

fn wire() -> WireBinding {
    WireBinding {
        method: "GET".into(),
        uri: vec![TemplatePart::Lit("/x".into())],
        bindings: Default::default(),
        response_bindings: Default::default(),
        success: Vec::new(),
        endpoint: None,
        request_headers: Vec::new(),
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

#[test]
fn success_expr_defaults_to_the_2xx_range_alone() {
    assert_eq!(
        success_expr(&wire()),
        "response.status >= 200 && response.status < 300"
    );
}

#[test]
fn success_expr_ors_in_only_the_out_of_range_declared_codes() {
    let mut w = wire();
    w.success = vec![200, 404, 202];
    // 200 and 202 are already covered by the range check; only 404 needs its
    // own arm, and in declaration order.
    assert_eq!(
        success_expr(&w),
        "response.status >= 200 && response.status < 300 || response.status === 404"
    );
}

#[test]
fn uri_expr_renders_a_pure_literal_without_a_template_wrapper() {
    let w = wire();
    assert_eq!(uri_expr(&w, &stub_field_expr), "\"/x\"");
}

#[test]
fn uri_expr_mixes_literal_field_and_input_placeholders() {
    let mut w = wire();
    w.uri = vec![
        TemplatePart::Lit("/notes/".into()),
        TemplatePart::Input("id".into()),
        TemplatePart::Lit("/".into()),
        TemplatePart::Field(vec!["region".into()]),
    ];
    assert_eq!(
        uri_expr(&w, &stub_field_expr),
        "`/notes/${pathPart(record[\"id\"])}/${pathPart(this.settings.region)}`"
    );
}

#[test]
fn body_expr_is_none_with_no_body_bound_members() {
    let mut w = wire();
    w.bindings = [("id".to_string(), WirePart::Label)].into_iter().collect();
    assert_eq!(body_expr(&w, "record"), None);
}

#[test]
fn body_expr_lists_only_the_body_members_when_mixed_with_other_kinds() {
    let mut w = wire();
    w.bindings = [
        ("id".to_string(), WirePart::Label),
        ("amount".to_string(), WirePart::Body),
        ("note".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        body_expr(&w, "record").as_deref(),
        Some("JSON.stringify({ \"amount\": record[\"amount\"], \"note\": record[\"note\"] })")
    );
}

#[test]
fn body_expr_serializes_the_encoded_input_directly_when_every_member_is_body() {
    let mut w = wire();
    w.bindings = [
        ("amount".to_string(), WirePart::Body),
        ("note".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    // No member carved out of `record` (no label/query/header/payload), so
    // stringifying the encoded input directly is both simpler than and, in
    // the presence of a `@wire` rename, more correct than re-listing every
    // field by its canonical name. needs_record agrees this case needs no
    // `record` alias, so the input expression is used as-is.
    assert_eq!(
        body_expr(&w, "encodeThing(input)").as_deref(),
        Some("JSON.stringify(encodeThing(input))")
    );
}

#[test]
fn body_expr_prefers_the_payload_member_over_any_body_members() {
    let mut w = wire();
    w.bindings = [
        ("envelope".to_string(), WirePart::Payload),
        ("extra".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        body_expr(&w, "record").as_deref(),
        Some("JSON.stringify(record[\"envelope\"])")
    );
}

#[test]
fn needs_record_is_false_with_no_bindings_at_all() {
    assert!(!needs_record(&wire()));
}

#[test]
fn needs_record_is_false_when_every_binding_is_a_body_member() {
    let mut w = wire();
    w.bindings = [
        ("amount".to_string(), WirePart::Body),
        ("note".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    assert!(!needs_record(&w));
}

#[test]
fn needs_record_is_true_when_a_label_binding_mixes_with_body_members() {
    let mut w = wire();
    w.uri = vec![TemplatePart::Input("id".into())];
    w.bindings = [
        ("id".to_string(), WirePart::Label),
        ("amount".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    assert!(needs_record(&w));
}

#[test]
fn needs_record_is_true_for_a_query_or_header_binding_alone() {
    let mut w = wire();
    w.bindings = [("tag".to_string(), WirePart::Query { name: "tag".into() })]
        .into_iter()
        .collect();
    assert!(needs_record(&w));
}

#[test]
fn has_query_and_query_lines_agree_on_query_bound_members() {
    let mut w = wire();
    assert!(!has_query(&w));
    assert_eq!(query_lines(&w, "  "), "");
    w.bindings = [("tag".to_string(), WirePart::Query { name: "tag".into() })]
        .into_iter()
        .collect();
    assert!(has_query(&w));
    assert_eq!(
        query_lines(&w, "  "),
        "  appendQuery(qs, \"tag\", record[\"tag\"]);\n"
    );
}

#[test]
#[should_panic(expected = "an entry @http op names its endpoint")]
fn endpoint_expr_with_no_declared_endpoint_is_an_emission_defect() {
    endpoint_expr(&wire(), &stub_field_expr);
}

#[test]
fn endpoint_expr_reads_the_typed_settings_field_with_no_runtime_guard() {
    let mut w = wire();
    w.endpoint = Some(vec!["endpoint".into()]);
    // The frontend guarantees `endpoint:` names a string field, so this
    // needs no `typeof`/`as string` guard and no fallback ternary: the bare
    // typed field access is the whole expression.
    assert_eq!(
        endpoint_expr(&w, &stub_field_expr),
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
        declared_header_lines(&w, "  ", &stub_field_expr),
        "  setHeader(headers, \"X-Client\", formatScalar(this.settings.client_name));\n"
    );
}

#[test]
fn per_call_header_lines_render_only_header_bound_members() {
    let mut w = wire();
    w.bindings = [
        ("id".to_string(), WirePart::Label),
        (
            "token".to_string(),
            WirePart::Header {
                name: "X-Api-Token".into(),
            },
        ),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        per_call_header_lines(&w, "  "),
        "  setHeader(headers, \"X-Api-Token\", formatScalar(record[\"token\"]));\n"
    );
}

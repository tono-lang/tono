//! Unit tests for the pure per-operation builders in `transport.rs`, split
//! from it to stay within the file-size gate. `entry/tests.rs` and the
//! codegen snapshot cover the assembled method end to end; these pin the
//! individual branches (label vs query vs payload, response bindings, the
//! all-body fast path, the retry/timeout pruning) that a fixture-level test
//! would only exercise one of at a time.

use super::*;
use crate::codegen::entries::module_entries;
use crate::codegen::targets::rust::types::rust_casing;
use crate::ir::{EntryField, Module, Shape, ShapeKind, Source};

fn wire() -> WireBinding {
    WireBinding {
        method: "GET".into(),
        uri: vec![TemplatePart::Lit("/x".into())],
        bindings: Default::default(),
        response_bindings: Default::default(),
        success: Vec::new(),
        endpoint: Some(vec!["endpoint".into()]),
        request_headers: Vec::new(),
        timeout: None,
        retry: None,
    }
}

fn field(name: &str, target: Tref) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources: vec![Source::Arg],
        format: None,
        transforms: vec![],
        select: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// A module whose single entry declares the fields the wire fixtures
/// reference, so a `FieldCtx` can resolve their types.
fn module() -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![
                    field("endpoint", Tref::Prim(Prim::String)),
                    field("api_key", Tref::Prim(Prim::String)),
                    field("region", Tref::Prim(Prim::I32)),
                    field("max_retries", Tref::Prim(Prim::I32)),
                ],
                operations: vec![],
            },
            traits: vec![],
        }],
        operations: vec![],
        extensions: vec![],
    }
}

/// Run `f` over a `FieldCtx` built on the fixture module (the ctx borrows
/// the entry model, which borrows the module, so the whole chain has to live
/// inside one scope).
fn with_ctx<R>(f: impl FnOnce(&FieldCtx<'_>) -> R) -> R {
    let module = module();
    let entries = module_entries(&module);
    let config = rust_casing();
    let ctx = FieldCtx {
        entry: &entries[0],
        module: &module,
        config: &config,
    };
    f(&ctx)
}

#[test]
fn success_expr_defaults_to_the_2xx_range_alone() {
    assert_eq!(
        success_expr(&wire()),
        "outcome.status >= 200 && outcome.status < 300"
    );
}

#[test]
fn success_expr_is_an_exact_match_against_declared_codes_only() {
    let mut w = wire();
    w.success = vec![200, 404, 202];
    assert_eq!(
        success_expr(&w),
        "outcome.status == 200 || outcome.status == 404 || outcome.status == 202"
    );
}

#[test]
fn success_expr_is_exact_for_a_single_declared_code_inside_2xx() {
    let mut w = wire();
    w.success = vec![201];
    assert_eq!(success_expr(&w), "outcome.status == 201");
}

#[test]
fn url_line_reads_the_typed_endpoint_and_percent_encodes_a_field_segment() {
    let mut w = wire();
    w.uri = vec![
        TemplatePart::Lit("/notes/".into()),
        TemplatePart::Input("id".into()),
        TemplatePart::Lit("/".into()),
        TemplatePart::Field(vec!["region".into()]),
    ];
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert_eq!(
        line,
        "let url = format!(\"{}/notes/{}/{}\", self.settings.endpoint, path_part(record.get(\"id\")), percent_path(&self.settings.region.to_string()));\n"
    );
}

#[test]
fn url_line_binds_mutably_only_when_a_query_will_be_appended() {
    let w = wire();
    assert!(with_ctx(|ctx| url_line(&w, true, ctx)).starts_with("let mut url"));
    assert!(with_ctx(|ctx| url_line(&w, false, ctx)).starts_with("let url"));
}

#[test]
#[should_panic(expected = "validate_entries rejects an entry @http op with no endpoint")]
fn url_line_with_no_declared_endpoint_is_an_emission_defect() {
    let mut w = wire();
    w.endpoint = None;
    with_ctx(|ctx| url_line(&w, false, ctx));
}

#[test]
fn body_lines_are_none_with_no_body_bound_members() {
    let mut w = wire();
    w.bindings = [("id".to_string(), WirePart::Label)].into_iter().collect();
    assert_eq!(body_lines(&w, true), None);
}

#[test]
fn body_lines_serialize_the_typed_input_directly_when_every_member_is_body() {
    let mut w = wire();
    w.bindings = [
        ("amount".to_string(), WirePart::Body),
        ("note".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    let lines = body_lines(&w, true).unwrap();
    assert!(lines.contains("serde_json::to_string(&input)"));
    assert!(!lines.contains("record"));
}

#[test]
fn body_lines_collect_only_the_body_members_when_mixed_with_other_kinds() {
    let mut w = wire();
    w.bindings = [
        ("id".to_string(), WirePart::Label),
        ("amount".to_string(), WirePart::Body),
        ("note".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    let lines = body_lines(&w, true).unwrap();
    assert!(lines.contains("for name in [\"amount\", \"note\"]"));
    assert!(lines.contains("body_members.insert(name.to_string(), v.clone())"));
}

#[test]
fn body_lines_prefer_the_payload_member_over_any_body_members() {
    let mut w = wire();
    w.bindings = [
        ("envelope".to_string(), WirePart::Payload),
        ("extra".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        body_lines(&w, true).as_deref(),
        Some("let body = record.get(\"envelope\").map(|v| v.to_string());\n")
    );
}

#[test]
fn needs_record_is_false_when_every_binding_is_a_body_member() {
    let mut w = wire();
    w.bindings = [("amount".to_string(), WirePart::Body)]
        .into_iter()
        .collect();
    assert!(!needs_record(&w));
    w.bindings
        .insert("tag".to_string(), WirePart::Query { name: "tag".into() });
    assert!(needs_record(&w));
}

#[test]
fn query_lines_append_each_query_bound_member_and_fold_conditionally() {
    let mut w = wire();
    w.bindings = [("tags".to_string(), WirePart::Query { name: "tag".into() })]
        .into_iter()
        .collect();
    let lines = query_lines(&w);
    assert!(lines.contains("append_query(&mut query, \"tag\", record.get(\"tags\"));"));
    assert!(lines.contains("if !query.is_empty()"));
}

#[test]
fn declared_header_lines_render_a_literal_key_with_a_field_value() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Client".into())],
        WireValue::Field(vec!["api_key".into()]),
    )];
    assert_eq!(
        with_ctx(|ctx| declared_header_lines(&w, ctx)),
        "set_header(&mut headers, \"X-Client\", self.settings.api_key.clone());\n"
    );
}

#[test]
fn per_call_header_lines_skip_an_absent_or_null_member_at_runtime() {
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
    let lines = per_call_header_lines(&w);
    assert!(lines.contains("if let Some(v) = record.get(\"token\")"));
    assert!(lines.contains("if !v.is_null()"));
    assert!(lines.contains("set_header(&mut headers, \"X-Api-Token\", format_scalar(v));"));
    assert!(!lines.contains("\"id\""));
}

#[test]
fn response_fold_lines_fold_only_on_the_exact_declared_success_path() {
    let mut w = wire();
    w.success = vec![200, 404];
    w.response_bindings = [
        (
            "trace".to_string(),
            WireResponsePart::Header {
                name: "X-Trace-Id".into(),
            },
        ),
        ("code".to_string(), WireResponsePart::StatusCode),
    ]
    .into_iter()
    .collect();
    let lines = response_fold_lines(&w);
    // The fold's own success test matches the classification below it: an
    // exact match against the declared codes, spelled against `response`.
    assert!(lines.contains("if response.status == 200 || response.status == 404 {"));
    assert!(lines.contains(
        "object.insert(\"code\".to_string(), serde_json::Value::from(response.status));"
    ));
    assert!(lines.contains("response.headers.get(\"x-trace-id\")"));
    assert!(lines.contains("} else {\n    response\n};"));
}

#[test]
fn retry_or_collapses_to_the_bare_failure_without_retry() {
    assert_eq!(retry_or(false, None, "Err(err)"), "Err(err)\n");
    let with = retry_or(true, Some("err.retryable()"), "return Err(err);");
    assert!(with.contains("if attempt < max_retries && err.retryable() {"));
    assert!(with.contains("(self.sleep)(backoff_delay_ms(attempt, (self.random)())).await;"));
    assert!(with.contains("attempt += 1;"));
}

#[test]
fn op_call_without_retry_timeout_or_hooks_carries_no_trace_of_them() {
    let w = wire();
    let mut refs = Vec::new();
    let out = with_ctx(|ctx| {
        op_call(
            &OpCall {
                wire: &w,
                method: "GET",
                has_input: false,
                has_declared_errors: false,
                discriminator: "decode_x_error",
                success_block: "Ok(())",
                before_request: None,
                after_response: None,
                timeout_field: None,
            },
            ctx,
            &mut refs,
        )
    });
    assert!(!out.contains("loop {"));
    assert!(!out.contains("max_retries"));
    assert!(!out.contains("self.sleep"));
    assert!(!out.contains("http_send_with_timeout"));
    assert!(!out.contains("attempt"));
    assert!(out.contains("http_send(&self.options, request)"));
    // The straight-line failure sits in tail position, never a bare
    // `return` as the method's last statement.
    assert!(out.trim_end().ends_with(
        "Err(TonoError::Api(APIFailure::Undeclared(APIError { status: outcome.status, body: outcome.body })))"
    ));
}

#[test]
fn op_call_with_retry_and_timeout_wraps_the_attempt_in_a_loop() {
    let mut w = wire();
    w.retry = Some(vec!["max_retries".into()]);
    let mut refs = Vec::new();
    let out = with_ctx(|ctx| {
        op_call(
            &OpCall {
                wire: &w,
                method: "POST",
                has_input: false,
                has_declared_errors: true,
                discriminator: "decode_x_error",
                success_block: "Ok(())",
                before_request: None,
                after_response: None,
                timeout_field: Some("timeout_ms".into()),
            },
            ctx,
            &mut refs,
        )
    });
    assert!(
        out.contains("let max_retries = resolve_max_retries(self.settings.max_retries as i64);")
    );
    assert!(out.contains("let mut attempt: u32 = 0;"));
    assert!(out.contains("loop {"));
    assert!(out.contains("http_send_with_timeout(&self.options, request, self.timeout_ms)"));
    assert!(out.contains("let err = decode_x_error(outcome.status, &outcome.body);"));
    assert!(out.contains("if attempt < max_retries && err.retryable() {"));
    // Both failure paths inside the loop return explicitly.
    assert!(out.contains("return Err(TonoError::Transport(TransportError { cause }));"));
    assert!(out.contains("return Err(err);"));
}

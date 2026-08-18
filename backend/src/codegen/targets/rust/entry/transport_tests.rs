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
        uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
        body: None,
        response_bindings: Default::default(),
        success: Vec::new(),
        endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
        request_headers: Vec::new(),
        query: vec![],
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
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// A module whose single entry declares the fields the wire fixtures
/// reference, so a `FieldCtx` can resolve their types. A `charge` structure
/// (unreferenced by the entry) stands in for an op's parameter type, so the
/// resolved-param-member tests have a same-module structure to resolve
/// against.
fn module() -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![
            Shape {
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
            },
            Shape {
                id: "m#charge".into(),
                kind: ShapeKind::Structure {
                    params: vec![],
                    members: vec![
                        crate::ir::Member {
                            name: "id".into(),
                            target: Tref::Prim(Prim::String),
                            required: true,
                            default: None,
                            constraints: vec![],
                            traits: vec![],
                        },
                        crate::ir::Member {
                            name: "created_at".into(),
                            target: Tref::Prim(Prim::Timestamp),
                            required: true,
                            default: None,
                            constraints: vec![],
                            traits: vec![],
                        },
                    ],
                },
                traits: vec![],
            },
        ],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
    }
}

/// Run `f` over a `FieldCtx` built on the fixture module (the ctx borrows
/// the entry model, which borrows the module, so the whole chain has to live
/// inside one scope). `input`, when set, is the op's own parameter type,
/// resolving a `Param(segs)` member through [`FieldCtx::param`] against the
/// fixture's `charge` structure.
fn with_ctx_and_input<R>(input: Option<Tref>, f: impl FnOnce(&FieldCtx<'_>) -> R) -> R {
    let module = module();
    let entries = module_entries(&module);
    let config = rust_casing();
    let ctx = FieldCtx {
        entry: &entries[0],
        module: &module,
        config: &config,
        input: input.as_ref(),
    };
    f(&ctx)
}

fn with_ctx<R>(f: impl FnOnce(&FieldCtx<'_>) -> R) -> R {
    with_ctx_and_input(None, f)
}

fn charge_ref() -> Tref {
    Tref::Ref {
        id: "m#charge".into(),
        args: vec![],
    }
}

// The range-vs-exact-match logic itself is proven once, target-agnostically,
// by `success_test_expr`'s own tests in `codegen::entries::wire`; this only
// pins the Rust field/operator wiring (`outcome.status`, `==`).
#[test]
fn success_expr_spells_the_rust_field_and_operator() {
    let mut w = wire();
    w.success = vec![200, 404];
    assert_eq!(
        success_expr(&w),
        "outcome.status == 200 || outcome.status == 404"
    );
}

#[test]
fn url_line_reads_the_typed_endpoint_and_percent_encodes_a_field_segment() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/notes/".into()),
        TemplatePart::Input("id".into()),
        TemplatePart::Lit("/".into()),
        TemplatePart::Field(vec!["region".into()]),
    ]);
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

// ── Named op-parameter references (WireValue::Param / TemplatePart::Param) ─

#[test]
fn a_bare_param_reference_in_a_path_template_reads_the_whole_input() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec![]),
    ]);
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains("percent_path(&input.to_string())"));
}

#[test]
fn a_param_member_reference_in_a_path_template_reads_off_the_record() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains("path_part(record.get(\"id\"))"));
}

// ── A resolvable param member reads straight off the typed input, and the
//    record disappears entirely when nothing else in the operation needs it.

#[test]
fn a_resolved_param_member_in_a_path_template_reads_the_typed_field() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    let line = with_ctx_and_input(Some(charge_ref()), |ctx| url_line(&w, false, ctx));
    assert!(line.contains("percent_path(&input.id)"));
    assert!(!line.contains("record"));
}

#[test]
fn a_resolved_branded_param_member_unwraps_before_percent_path() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["created_at".into()]),
    ]);
    let line = with_ctx_and_input(Some(charge_ref()), |ctx| url_line(&w, false, ctx));
    assert!(line.contains("percent_path(&input.created_at.0)"));
}

#[test]
fn a_resolved_param_member_in_a_header_value_reads_the_typed_field() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec!["id".into()]),
    )];
    let out = with_ctx_and_input(Some(charge_ref()), |ctx| declared_header_lines(&w, ctx));
    assert!(out.contains("input.id.clone()"));
    assert!(!out.contains("record"));
}

#[test]
fn a_resolved_param_member_in_a_query_value_reads_the_typed_field() {
    let mut w = wire();
    w.query = vec![(
        vec![TemplatePart::Lit("id".into())],
        WireValue::Param(vec!["id".into()]),
    )];
    let out = with_ctx_and_input(Some(charge_ref()), |ctx| query_lines(&w, ctx));
    assert!(out.contains("serde_json::to_value(&input.id).unwrap_or(serde_json::Value::Null)"));
    assert!(!out.contains("record"));
}

#[test]
fn a_cross_module_param_type_falls_back_to_the_record() {
    let mut w = wire();
    w.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    let other_module_ref = Tref::Ref {
        id: "other#charge".into(),
        args: vec![],
    };
    let line = with_ctx_and_input(Some(other_module_ref), |ctx| url_line(&w, false, ctx));
    assert!(line.contains("path_part(record.get(\"id\"))"));
}

#[test]
fn a_pure_param_reference_in_path_position_passes_through_unescaped() {
    let mut w = wire();
    w.uri = WireValue::Param(vec![]);
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains("input.to_string()"));
    assert!(!line.contains("percent_path"));
}

#[test]
fn a_pure_param_member_reference_in_path_position_passes_through_unescaped() {
    let mut w = wire();
    w.uri = WireValue::Param(vec!["href".into()]);
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains(
        "format_scalar(record.get(\"href\").unwrap_or(&serde_json::Value::Null)).to_string()"
    ));
}

#[test]
fn a_literal_uri_is_a_plain_string() {
    let mut w = wire();
    w.uri = WireValue::Lit(serde_json::json!("/fixed"));
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains("\"/fixed\".to_string()"));
}

#[test]
fn a_non_field_endpoint_goes_through_the_general_renderer() {
    let mut w = wire();
    w.endpoint = Some(WireValue::Lit(serde_json::json!("https://example.com")));
    let line = with_ctx(|ctx| url_line(&w, false, ctx));
    assert!(line.contains("\"https://example.com\".to_string()"));
}

#[test]
fn a_param_reference_in_a_header_value_reads_the_whole_input() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec![]),
    )];
    let out = with_ctx(|ctx| declared_header_lines(&w, ctx));
    assert!(out.contains("input.to_string()"));
}

#[test]
fn a_param_reference_in_a_header_key_reads_off_the_record() {
    let mut w = wire();
    w.request_headers = vec![(
        vec![TemplatePart::Param(vec!["region".into()])],
        WireValue::Lit(serde_json::json!("v")),
    )];
    let out = with_ctx(|ctx| declared_header_lines(&w, ctx));
    assert!(out.contains("record.get(\"region\")"));
}

#[test]
fn body_lines_are_none_with_no_body_declared() {
    let w = wire();
    assert_eq!(with_ctx(|ctx| body_lines(&w, ctx)), None);
}

#[test]
fn body_lines_serialize_the_typed_input_directly_for_the_whole_param() {
    let mut w = wire();
    w.body = Some(WireValue::Param(vec![]));
    let lines = with_ctx(|ctx| body_lines(&w, ctx)).unwrap();
    assert!(lines.contains("serde_json::to_string(&input)"));
    assert!(!lines.contains("record"));
}

#[test]
fn body_lines_build_the_ctor_mapper_object_when_body_is_an_object() {
    let mut w = wire();
    w.body = Some(WireValue::Object(vec![
        (
            "amount".to_string(),
            WireValue::Param(vec!["amount".into()]),
        ),
        ("note".to_string(), WireValue::Param(vec!["note".into()])),
    ]));
    let lines = with_ctx(|ctx| body_lines(&w, ctx)).unwrap();
    assert!(lines.contains("m.insert(\"amount\".to_string(), record.get(\"amount\")"));
    assert!(lines.contains("m.insert(\"note\".to_string(), record.get(\"note\")"));
    assert!(lines.contains("serde_json::Value::Object(m)"));
}

#[test]
fn body_lines_read_one_member_raw_for_a_param_member_body() {
    let mut w = wire();
    w.body = Some(WireValue::Param(vec!["envelope".into()]));
    assert_eq!(
        with_ctx(|ctx| body_lines(&w, ctx)),
        Some("let body = record.get(\"envelope\").map(|v| v.to_string());\n".to_string())
    );
}

#[test]
fn needs_record_is_false_with_no_body_and_true_for_a_query() {
    let mut w = wire();
    assert!(!needs_record(&w));
    w.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tags".into()]),
    )];
    assert!(needs_record(&w));
}

#[test]
fn query_lines_append_each_declared_query_and_fold_conditionally() {
    let mut w = wire();
    w.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tags".into()]),
    )];
    let lines = with_ctx(|ctx| query_lines(&w, ctx));
    assert!(lines.contains(
        "append_query(&mut query, \"tag\", Some(&record.get(\"tags\").cloned().unwrap_or(serde_json::Value::Null)));"
    ));
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
fn op_call_without_retry_or_timeout_carries_no_trace_of_them() {
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

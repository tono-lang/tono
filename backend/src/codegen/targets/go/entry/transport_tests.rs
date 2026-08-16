use super::*;
use crate::codegen::casing::CaseStyle;
use crate::codegen::targets::go::GoRules;
use crate::codegen::tree::Decl;
use crate::ir::{Module, WireBinding, WireValue};

fn empty_module() -> Module {
    Module {
        name: "m".into(),
        shapes: vec![],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
        tests: vec![],
    }
}

fn base_wire() -> WireBinding {
    WireBinding {
        method: "POST".into(),
        uri: WireValue::Template(vec![TemplatePart::Lit("/charges".into())]),
        body: Some(WireValue::Param(vec![])),
        response_bindings: Default::default(),
        success: Vec::new(),
        endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
        request_headers: Vec::new(),
        query: vec![],
        timeout: None,
        retry: None,
    }
}

struct Case {
    wire: WireBinding,
    discriminator: Option<&'static str>,
    retry_expr: Option<String>,
    timeout_expr: Option<String>,
    /// Param members the target can resolve through typed field access
    /// (name, Go field identifier, kind); empty means every param member
    /// falls back to the decoded record, matching every other case in this
    /// file.
    resolved_params: Vec<(&'static str, &'static str, FieldKind)>,
    /// Overrides `empty_module()` when a case needs `ext_libs` (a
    /// call-valued header/body reads the module's own extern declarations).
    module: Option<Module>,
}

impl Case {
    fn new(wire: WireBinding) -> Self {
        Case {
            wire,
            discriminator: None,
            retry_expr: None,
            timeout_expr: None,
            resolved_params: Vec::new(),
            module: None,
        }
    }

    /// The rendered method body: slots spelled the way the engine spells them,
    /// so the assertions read what the generated file would carry.
    fn text(&self) -> String {
        let fail = |expr: String| expr;
        let field_access = |path: &[String]| format!("c.settings.{}", path.join("."));
        let field_kind = |_: &[String]| FieldKind::StringLike;
        let param_access = |name: &str| {
            self.resolved_params
                .iter()
                .find(|(n, _, _)| *n == name)
                .map(|(_, field, kind)| (field.to_string(), *kind))
        };
        let mut refs = Vec::new();
        let module = self.module.clone().unwrap_or_else(empty_module);
        let config = CasingConfig::new(CaseStyle::Camel);
        let call = OpCall {
            wire: &self.wire,
            module: &module,
            config: &config,
            has_input: true,
            ret_zero: "zero, ",
            discriminator: self.discriminator,
            api_error: "APIError",
            transport_error: "TransportError",
            success_block: "\treturn out, nil",
            retry_expr: self.retry_expr.clone(),
            timeout_expr: self.timeout_expr.clone(),
        };
        let body = op_call(
            &call,
            &fail,
            &field_access,
            &field_kind,
            &param_access,
            &mut refs,
        );
        crate::codegen::test_support::rendered(&[Decl::raw_with(body, refs)], &GoRules::default())
    }
}

#[test]
fn a_plain_operation_carries_no_retry_or_timeout_piece() {
    let out = Case::new(base_wire()).text();
    assert!(out.contains(
        "outcome := transport.Send(ctx, c.settings.HTTPClient, c.settings.Transport, transport.Request{"
    ));
    assert!(out.contains("Method: \"POST\","));
    assert!(out.contains("requestURL := c.settings.endpoint + \"/charges\""));
    assert!(!out.contains("Retry"));
    assert!(!out.contains("Timing"));
    assert!(!out.contains("Timeout"));
    // The all-body input marshals directly; no record indirection.
    assert!(out.contains("body, err := json.Marshal(input)"));
    assert!(!out.contains("EncodeRecord"));
}

#[test]
fn a_retrying_operation_builds_the_predicate_from_its_own_discriminator() {
    let mut case = Case::new(base_wire());
    case.wire.retry = Some(vec!["max_retries".into()]);
    case.retry_expr = Some("int(c.settings.MaxRetries)".into());
    case.discriminator = Some("DecodeCreateChargeError");
    let out = case.text();
    assert!(out.contains("Timing: c.timing,"));
    assert!(out.contains(
        "Retry: transport.Retry{Max: int(c.settings.MaxRetries), When: func(status int, body string) bool {"
    ));
    assert!(out.contains(
        "if re, ok := DecodeCreateChargeError(status, []byte(body)).(interface{ Retryable() bool }); ok {"
    ));
}

#[test]
fn a_retrying_operation_with_no_declared_errors_carries_no_predicate() {
    let mut case = Case::new(base_wire());
    case.wire.retry = Some(vec!["max_retries".into()]);
    case.retry_expr = Some("int(c.settings.MaxRetries)".into());
    let out = case.text();
    assert!(out.contains("Retry: transport.Retry{Max: int(c.settings.MaxRetries)},"));
    assert!(!out.contains("When:"));
}

#[test]
fn a_timeout_operation_reads_the_preconverted_client_field() {
    let mut case = Case::new(base_wire());
    case.wire.timeout = Some(vec!["timeout".into()]);
    case.timeout_expr = Some("c.timeoutDuration".into());
    let out = case.text();
    assert!(out.contains("Timeout: c.timeoutDuration,"));
}

#[test]
fn a_labeled_path_reads_the_record_and_url_encodes() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/things/".into()),
        TemplatePart::Input("id".into()),
    ]);
    case.wire.body = None;
    let out = case.text();
    assert!(out.contains("record, err := record.EncodeRecord(input)"));
    assert!(out.contains(
        "requestURL := c.settings.endpoint + \"/things/\" + transport.PathPart(record[\"id\"])"
    ));
    // No @body declared, so no body is sent and no content-type defaults in.
    assert!(!out.contains("json.Marshal"));
    assert!(!out.contains("content-type"));
}

#[test]
fn a_query_binding_collects_entries_in_order_and_folds_the_tail() {
    let mut case = Case::new(base_wire());
    case.wire.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tag".into()]),
    )];
    case.wire.body = Some(WireValue::Object(vec![(
        "amount".to_string(),
        WireValue::Param(vec!["amount".into()]),
    )]));
    let out = case.text();
    assert!(out.contains("var query []string"));
    assert!(out.contains("query = transport.AppendQuery(query, \"tag\", record[\"tag\"])"));
    assert!(out.contains("+ transport.QueryString(query)"));
    // The @body ctor mapper builds an object of just its declared field.
    assert!(
        out.contains("body, err := json.Marshal(map[string]any{\"amount\": record[\"amount\"]})")
    );
    assert!(out.contains("if !transport.HasHeader(headers, \"content-type\")"));
}

#[test]
fn a_param_member_body_is_the_whole_body_read_raw_off_the_record() {
    let mut case = Case::new(base_wire());
    case.wire.body = Some(WireValue::Param(vec!["data".into()]));
    let out = case.text();
    assert!(out.contains("if v, ok := record[\"data\"]; ok {"));
    assert!(out.contains("encoded, err := json.Marshal(v)"));
    assert!(out.contains("if body != nil && !transport.HasHeader(headers, \"content-type\")"));
}

#[test]
fn headers_layer_declared_then_base() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-API-Key".into())],
        WireValue::Field(vec!["api_key".into()]),
    )];
    let out = case.text();
    let declared = out
        .find("transport.SetHeader(headers, \"X-API-Key\", c.settings.api_key)")
        .expect("declared header");
    let base = out
        .find("for name, value := range c.settings.Headers {")
        .expect("base header layer");
    assert!(declared < base);
}

#[test]
fn response_bindings_fold_only_on_the_success_path() {
    let mut case = Case::new(base_wire());
    case.wire.response_bindings = [
        ("code".to_string(), WireResponsePart::StatusCode),
        (
            "trace".to_string(),
            WireResponsePart::Header {
                name: "X-Trace".into(),
            },
        ),
    ]
    .into_iter()
    .collect();
    let out = case.text();
    assert!(out.contains(
        "folded := transport.FoldResponse(outcome.Body, map[string]any{\"code\": outcome.Status, \"trace\": transport.HeaderValue(outcome.Headers, \"x-trace\")})"
    ));
    // The fold sits inside the success check; the error path reads the raw
    // outcome.
    let fold_at = out.find("folded :=").expect("fold");
    let success_at = out
        .find("if outcome.Status >= 200 && outcome.Status < 300 {")
        .expect("success check");
    assert!(success_at < fold_at);
}

#[test]
fn declared_success_codes_make_both_checks_exact() {
    let mut case = Case::new(base_wire());
    case.wire.success = vec![200, 409];
    case.wire.retry = Some(vec!["max_retries".into()]);
    case.retry_expr = Some("int(c.settings.MaxRetries)".into());
    let out = case.text();
    assert!(out.contains("if outcome.Status == 200 || outcome.Status == 409 {"));
    assert!(out.contains("Success: []int{200, 409},"));
}

#[test]
fn without_retry_the_success_list_stays_out() {
    let mut case = Case::new(base_wire());
    case.wire.success = vec![200, 409];
    let out = case.text();
    assert!(out.contains("outcome.Status == 200 || outcome.Status == 409"));
    assert!(!out.contains("Success:"));
}

#[test]
fn the_outcome_maps_onto_the_taxonomy() {
    let mut case = Case::new(base_wire());
    case.discriminator = Some("DecodeCreateChargeError");
    let out = case.text();
    assert!(out.contains("if outcome.Cause != nil {"));
    assert!(out.contains("&TransportError{Cause: outcome.Cause}"));
    assert!(
        out.contains("return zero, DecodeCreateChargeError(outcome.Status, []byte(outcome.Body))")
    );
}

#[test]
fn an_operation_with_no_declared_errors_returns_the_generic_api_error() {
    let out = Case::new(base_wire()).text();
    assert!(out.contains("return zero, &APIError{Status: outcome.Status, Body: outcome.Body}"));
}

#[test]
#[should_panic(expected = "validate_entries rejects an entry @http op with no endpoint")]
fn a_missing_endpoint_is_an_emission_defect_behind_the_validator() {
    let mut case = Case::new(base_wire());
    case.wire.endpoint = None;
    case.text();
}

// ── Named op-parameter references (WireValue::Param / TemplatePart::Param) ─

#[test]
fn a_bare_param_reference_in_a_path_template_reads_the_whole_input() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec![]),
    ]);
    let out = case.text();
    assert!(out.contains("transport.PathPart(input)"));
}

#[test]
fn a_param_member_reference_in_a_path_template_reads_off_the_record() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    let out = case.text();
    assert!(out.contains("transport.PathPart(record[\"id\"])"));
}

// ── A resolvable param member reads straight off the typed input, and the
//    record disappears entirely when nothing else in the operation needs it.

#[test]
fn a_resolved_param_member_in_a_path_template_reads_the_typed_field() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    case.resolved_params = vec![("id", "ID", FieldKind::StringLike)];
    let out = case.text();
    assert!(out.contains("transport.PathPart(input.ID)"));
    assert!(!out.contains("record"));
    assert!(!out.contains("EncodeRecord"));
}

#[test]
fn a_resolved_branded_param_member_flattens_before_pathpart() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["kind".into()]),
    ]);
    case.resolved_params = vec![("kind", "Kind", FieldKind::Branded)];
    let out = case.text();
    assert!(out.contains("transport.PathPart(string(input.Kind))"));
}

#[test]
fn a_resolved_param_member_in_a_header_value_skips_formatscalar_when_stringlike() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec!["id".into()]),
    )];
    case.resolved_params = vec![("id", "ID", FieldKind::StringLike)];
    let out = case.text();
    assert!(out.contains("transport.SetHeader(headers, \"X-Id\", input.ID)"));
    assert!(!out.contains("record"));
}

#[test]
fn a_resolved_other_kind_param_member_still_formats() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Amount".into())],
        WireValue::Param(vec!["amount".into()]),
    )];
    case.resolved_params = vec![("amount", "Amount", FieldKind::Other)];
    let out = case.text();
    assert!(out.contains("transport.FormatScalar(input.Amount)"));
}

#[test]
fn a_resolved_param_member_in_a_query_value_reads_the_typed_field() {
    let mut case = Case::new(base_wire());
    case.wire.body = None;
    case.wire.query = vec![(
        vec![TemplatePart::Lit("tag".into())],
        WireValue::Param(vec!["tag".into()]),
    )];
    case.resolved_params = vec![("tag", "Tag", FieldKind::StringLike)];
    let out = case.text();
    assert!(out.contains("transport.AppendQuery(query, \"tag\", input.Tag)"));
    assert!(!out.contains("record"));
}

#[test]
fn one_unresolved_member_still_needs_the_record_even_when_another_resolves() {
    let mut case = Case::new(base_wire());
    case.wire.body = None;
    case.wire.uri = WireValue::Template(vec![
        TemplatePart::Lit("/charges/".into()),
        TemplatePart::Param(vec!["id".into()]),
    ]);
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Other".into())],
        WireValue::Param(vec!["other".into()]),
    )];
    case.resolved_params = vec![("id", "ID", FieldKind::StringLike)];
    let out = case.text();
    assert!(out.contains("record, err := record.EncodeRecord(input)"));
    assert!(out.contains("transport.PathPart(input.ID)"));
    assert!(out.contains("transport.FormatScalar(record[\"other\"])"));
}

#[test]
fn a_pure_param_reference_in_path_position_passes_through_unescaped() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Param(vec![]);
    let out = case.text();
    assert!(out.contains("transport.FormatScalar(input)"));
    assert!(!out.contains("PathPart"));
}

#[test]
fn a_pure_param_member_reference_in_path_position_passes_through_unescaped() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Param(vec!["href".into()]);
    let out = case.text();
    assert!(out.contains("transport.FormatScalar(record[\"href\"])"));
}

#[test]
fn a_literal_uri_is_a_plain_string() {
    let mut case = Case::new(base_wire());
    case.wire.uri = WireValue::Lit(serde_json::json!("/fixed"));
    let out = case.text();
    assert!(out.contains("\"/fixed\""));
}

#[test]
fn a_param_reference_in_a_header_value_reads_the_whole_input() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Id".into())],
        WireValue::Param(vec![]),
    )];
    let out = case.text();
    assert!(out.contains("transport.FormatScalar(input)"));
}

#[test]
fn a_param_reference_in_a_header_key_reads_off_the_record() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Param(vec!["region".into()])],
        WireValue::Lit(serde_json::json!("v")),
    )];
    let out = case.text();
    assert!(out.contains("transport.FormatScalar(record[\"region\"])"));
}

/// A module declaring `companyauth.sign`, the ext block a call-valued
/// header/body reads to resolve its callee.
fn module_with_sign_extern() -> Module {
    use crate::ir::{CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, LangPath, Prim, Tref};
    let mut module = empty_module();
    module.ext_libs = vec![ExtLib {
        name: "companyauth".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "company/auth".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "sign".into(),
            params: vec![ExternParam {
                name: "request".into(),
                r#type: Tref::Prim(Prim::String),
            }],
            r#return: Tref::Prim(Prim::String),
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Sign".into(),
                call_args: vec![CallArg::Ref(vec!["request".into()])],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: true,
                infallible: false,
            }],
        }],
    }];
    module
}

fn sign_call() -> WireValue {
    use crate::ir::WireCall;
    WireValue::Call(WireCall {
        ns: "companyauth".into(),
        fn_name: "sign".into(),
        args: vec![crate::ir::WireCallArg::Request],
    })
}

#[test]
fn a_call_valued_header_invokes_the_extern_before_set_header() {
    let mut case = Case::new(base_wire());
    case.module = Some(module_with_sign_extern());
    case.wire.request_headers =
        vec![(vec![TemplatePart::Lit("Authorization".into())], sign_call())];
    let out = case.text();
    assert!(out.contains("signed0, signed0Err :="), "{out}");
    assert!(out.contains(".Sign(req)"), "{out}");
    assert!(out.contains("signed0)"), "{out}");
}

#[test]
fn a_call_valued_body_assigns_the_extern_result_to_req_body() {
    let mut case = Case::new(base_wire());
    case.module = Some(module_with_sign_extern());
    case.wire.body = Some(sign_call());
    let out = case.text();
    assert!(out.contains("signedBody, signedBodyErr :="), "{out}");
    assert!(out.contains("req.Body = []byte(signedBody)"), "{out}");
}

#[test]
fn no_call_valued_header_or_body_emits_no_call_lines() {
    let case = Case::new(base_wire());
    let out = case.text();
    assert!(!out.contains("Err :="));
}

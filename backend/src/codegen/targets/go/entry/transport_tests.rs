use super::*;
use crate::codegen::targets::go::GoRules;
use crate::codegen::tree::Decl;
use crate::ir::WireBinding;

fn base_wire() -> WireBinding {
    WireBinding {
        method: "POST".into(),
        uri: vec![TemplatePart::Lit("/charges".into())],
        bindings: [("amount".to_string(), WirePart::Body)]
            .into_iter()
            .collect(),
        response_bindings: Default::default(),
        success: vec![200],
        endpoint: Some(vec!["endpoint".into()]),
        request_headers: Vec::new(),
        timeout: None,
        retry: None,
    }
}

struct Case {
    wire: WireBinding,
    discriminator: Option<&'static str>,
    module_hooks: bool,
    retry_expr: Option<String>,
    timeout_expr: Option<String>,
}

impl Case {
    fn new(wire: WireBinding) -> Self {
        Case {
            wire,
            discriminator: None,
            module_hooks: false,
            retry_expr: None,
            timeout_expr: None,
        }
    }

    /// The rendered method body: slots spelled the way the engine spells them,
    /// so the assertions read what the generated file would carry.
    fn text(&self) -> String {
        let fail = |expr: String| expr;
        let field_access = |path: &[String]| format!("c.settings.{}", path.join("."));
        let field_kind = |_: &[String]| FieldKind::StringLike;
        let mut refs = Vec::new();
        let call = OpCall {
            wire: &self.wire,
            has_input: true,
            ret_zero: "zero, ",
            discriminator: self.discriminator,
            api_error: "APIError",
            transport_error: "TransportError",
            success_block: "\treturn out, nil",
            module_hooks: self.module_hooks,
            retry_expr: self.retry_expr.clone(),
            timeout_expr: self.timeout_expr.clone(),
        };
        let body = op_call(&call, &fail, &field_access, &field_kind, &mut refs);
        crate::codegen::test_support::rendered(&[Decl::raw_with(body, refs)], &GoRules::default())
    }
}

#[test]
fn a_plain_operation_carries_no_retry_timeout_or_hook_piece() {
    let out = Case::new(base_wire()).text();
    assert!(out.contains(
        "outcome := transport.Send(ctx, c.settings.HTTPClient, c.settings.Transport, transport.Request{"
    ));
    assert!(out.contains("Method: \"POST\","));
    assert!(!out.contains("HookErr"));
    assert!(out.contains("requestURL := c.settings.endpoint + \"/charges\""));
    assert!(!out.contains("Retry"));
    assert!(!out.contains("Timing"));
    assert!(!out.contains("Timeout"));
    assert!(!out.contains("Hooks"));
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
fn a_bound_module_hook_rides_the_request() {
    let mut case = Case::new(base_wire());
    case.module_hooks = true;
    let out = case.text();
    assert!(out.contains("Hooks: c.hooks,"));
    // The hook-failure check exists exactly where a hook is bound.
    assert!(out.contains("if outcome.HookErr != nil {"));
}

#[test]
fn a_labeled_path_reads_the_record_and_url_encodes() {
    let mut case = Case::new(base_wire());
    case.wire.uri = vec![
        TemplatePart::Lit("/things/".into()),
        TemplatePart::Input("id".into()),
    ];
    case.wire.bindings = [("id".to_string(), WirePart::Label)].into_iter().collect();
    let out = case.text();
    assert!(out.contains("record, err := record.EncodeRecord(input)"));
    assert!(out.contains(
        "requestURL := c.settings.endpoint + \"/things/\" + transport.PathPartRaw(record[\"id\"])"
    ));
    // Every binding left the body, so no body is sent and no content-type
    // defaults in.
    assert!(!out.contains("json.Marshal"));
    assert!(!out.contains("content-type"));
}

#[test]
fn a_query_binding_collects_entries_in_order_and_folds_the_tail() {
    let mut case = Case::new(base_wire());
    case.wire.bindings = [
        ("tag".to_string(), WirePart::Query { name: "tag".into() }),
        ("amount".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    let out = case.text();
    assert!(out.contains("var query []string"));
    assert!(out.contains("query = transport.AppendQuery(query, \"tag\", record[\"tag\"])"));
    assert!(out.contains("+ transport.QueryString(query)"));
    // A mixed body assembles just the body members, in member order.
    assert!(out.contains("body := transport.EncodeBody(record, \"amount\")"));
    assert!(out.contains("if body != nil && !transport.HasHeader(headers, \"content-type\")"));
}

#[test]
fn a_payload_member_is_the_whole_body_and_absence_sends_none() {
    let mut case = Case::new(base_wire());
    case.wire.bindings = [("data".to_string(), WirePart::Payload)]
        .into_iter()
        .collect();
    let out = case.text();
    // The record already holds the member's raw bytes: no re-encode.
    assert!(out.contains("if v, ok := record[\"data\"]; ok {"));
    assert!(out.contains("body = v"));
    assert!(!out.contains("json.Marshal(v)"));
    assert!(out.contains("if body != nil && !transport.HasHeader(headers, \"content-type\")"));
}

#[test]
fn headers_layer_declared_then_base_then_per_call() {
    let mut case = Case::new(base_wire());
    case.wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-API-Key".into())],
        WireValue::Field(vec!["api_key".into()]),
    )];
    case.wire.bindings = [
        (
            "trace".to_string(),
            WirePart::Header {
                name: "X-Trace".into(),
            },
        ),
        ("amount".to_string(), WirePart::Body),
    ]
    .into_iter()
    .collect();
    let out = case.text();
    let declared = out
        .find("transport.SetHeader(headers, \"X-API-Key\", c.settings.api_key)")
        .expect("declared header");
    let base = out
        .find("for name, value := range c.settings.Headers {")
        .expect("base header layer");
    let per_call = out
        .find("if v, ok := record[\"trace\"]; ok && string(v) != \"null\" {")
        .expect("per-call header guard");
    assert!(declared < base && base < per_call);
    assert!(out.contains("transport.SetHeader(headers, \"X-Trace\", transport.FormatRaw(v))"));
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
        "folded := transport.FoldResponse(outcome.Body, map[string]json.RawMessage{\"code\": json.RawMessage(strconv.Itoa(outcome.Status)), \"trace\": transport.HeaderValue(outcome.Headers, \"x-trace\")})"
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
fn a_declared_out_of_range_success_status_extends_both_checks() {
    let mut case = Case::new(base_wire());
    case.wire.success = vec![200, 409];
    case.wire.retry = Some(vec!["max_retries".into()]);
    case.retry_expr = Some("int(c.settings.MaxRetries)".into());
    let out = case.text();
    assert!(
        out.contains("if outcome.Status >= 200 && outcome.Status < 300 || outcome.Status == 409 {")
    );
    assert!(out.contains("Success: []int{409},"));
}

#[test]
fn without_retry_the_extra_success_list_stays_out() {
    let mut case = Case::new(base_wire());
    case.wire.success = vec![200, 409];
    let out = case.text();
    assert!(out.contains("|| outcome.Status == 409"));
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

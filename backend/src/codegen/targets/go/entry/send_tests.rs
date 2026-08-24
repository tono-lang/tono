use super::*;
use crate::codegen::targets::go::GoRules;
use crate::ir::{Shape, ShapeKind, WireBinding, WireValue};

fn package_text(usage: &Usage) -> String {
    crate::codegen::test_support::rendered(&internal_helpers(usage), &GoRules::default())
}

#[test]
fn a_bare_usage_emits_a_transport_with_no_retry_or_timeout_piece() {
    let out = package_text(&Usage::default());
    // Send is the single attempt itself: no loop, no policy fields, no seams.
    assert!(out.contains("func Send(ctx context.Context, native *http.Client, canonical support.HTTPTransport, req Request) Outcome {"));
    assert!(!out.contains("for attempt"));
    assert!(!out.contains("sendOnce"));
    assert!(!out.contains("type Retry struct"));
    assert!(!out.contains("type Timing struct"));
    assert!(!out.contains("backoffDelay"));
    assert!(!out.contains("rand.Float64"));
    assert!(!out.contains("Timeout"));
    assert!(!out.contains("WithTimeout"));
    // The attempt still copies headers fresh and classifies a dispatch
    // failure as a transport outcome.
    assert!(out.contains("headers := make(map[string]string, len(req.Headers))"));
    assert!(out.contains("return Outcome{Cause: err}"));
}

#[test]
fn retry_usage_wraps_the_attempt_in_the_backoff_loop() {
    let usage = Usage {
        retry: true,
        ..Usage::default()
    };
    let out = package_text(&usage);
    assert!(out.contains("for attempt := 0; ; attempt++ {"));
    assert!(out.contains("outcome := sendOnce(ctx, native, canonical, req)"));
    assert!(out.contains("if attempt >= req.Retry.Max || !retryable(outcome, req) {"));
    // The parity contract's backoff constants.
    assert!(out.contains("exp := math.Min(2000, 100*math.Pow(2, float64(attempt)))"));
    // The timing seam defaults to the real clock and jitter.
    assert!(out.contains("random = rand.Float64"));
    assert!(out.contains("sleep = sleepContext"));
    // Still no timeout piece.
    assert!(!out.contains("Timeout"));
}

#[test]
fn timeout_usage_bounds_the_dispatch_with_a_context_deadline() {
    let usage = Usage {
        timeout: true,
        ..Usage::default()
    };
    let out = package_text(&usage);
    assert!(out.contains("Timeout time.Duration"));
    assert!(out.contains("if req.Timeout > 0 {"));
    assert!(out.contains("attemptCtx, cancel = context.WithTimeout(ctx, req.Timeout)"));
    assert!(out.contains("response, err := dispatch(attemptCtx, native, canonical, request)"));
}

#[test]
fn the_dispatch_prefers_the_canonical_transport_and_lowercases_headers() {
    let out = package_text(&Usage::default());
    assert!(out.contains("if canonical != nil {\n\t\treturn canonical(ctx, req)\n\t}"));
    assert!(out.contains("client = http.DefaultClient"));
    assert!(out.contains("headers[strings.ToLower(name)] = httpRes.Header.Get(name)"));
}

fn entry_module(retry: bool, timeout: bool) -> crate::ir::Module {
    let wire = WireBinding {
        method: "POST".into(),
        uri: WireValue::Template(vec![crate::ir::TemplatePart::Lit("/x".into())]),
        body: None,
        response_bindings: Default::default(),
        success: vec![200],
        endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
        request_headers: Vec::new(),
        query: vec![],
        timeout: timeout.then(|| vec!["timeout".into()]),
        retry: retry.then(|| vec!["max_retries".into()]),
    };
    let op = Shape {
        id: "m#client.call".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            output_nullable: false,
            errors: vec![],
            wire: Some(Box::new(wire)),
            impl_call: None,
        },
        traits: vec![],
    };
    crate::ir::Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![],
                operations: vec![op],
            },
            traits: vec![],
        }],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
    }
}

#[test]
fn usage_is_read_off_the_wire_bindings() {
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![entry_module(false, false)],
    };
    let usage = usage_of(&model);
    assert!(!usage.retry && !usage.timeout);

    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![entry_module(true, false), entry_module(false, true)],
    };
    let usage = usage_of(&model);
    assert!(usage.retry && usage.timeout);
}

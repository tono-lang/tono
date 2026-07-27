//! The bespoke-boundary half of the Go entry tests: bound hooks, the wrappers
//! they are called through, and an operation whose body is a bound `impl`. Split
//! from the construction and resolution tests so each file stays within the repo
//! size gate.

use super::tests::{entry_text, fixture_module};

#[test]
fn bound_hooks_wire_the_settings_bridge_and_the_transport_slots() {
    let mut module = fixture_module();
    module.extensions = vec![
        crate::ir::Extension {
            name: "client_init".into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            raw: false,
            bindings: [("go".to_string(), "ext/go/init.go#InitSettings".to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        },
        crate::ir::Extension {
            name: "before_request".into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            raw: false,
            bindings: [("go".to_string(), "ext/go/auth.go#AddBearer".to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        },
    ];
    let serde = entry_text(&module);
    // client_init runs over the resolved Settings, after sources and
    // before validation; a bespoke failure is a ContractError.
    assert!(serde.contains("func clientInitHook(s *Settings) error {"));
    assert!(serde.contains("if err := clientInitHook(&s); err != nil {"));
    assert!(serde.contains("ContractError{ContractName: \"client_init\", Cause: err}"));
    // The construction wires the transport slots and the resolved values.
    assert!(serde.contains(
            "tonohttp.New(tonohttp.Options{Client: s.HTTPClient, Transport: s.Transport, Headers: s.Headers, Values: values})"
        ));
    // before_request is handed to the runtime once per client.
    assert!(serde.contains("func beforeRequestHook(ctx context.Context, req tonohttp.CanonicalRequest) (tonohttp.CanonicalRequest, error) {"));
    assert!(serde.contains("hooks: &tonohttp.Hooks{BeforeRequest: beforeRequestHook}"));
    // The hook order lands in the emitted text: init before the requires.
    let init = serde.find("clientInitHook(&s)").unwrap();
    let require = serde
        .find("&ConfigError{Message: \"endpoint <- \"")
        .unwrap();
    assert!(init < require);
}

/// An `ext impl` binding for the fixture's `save_note`, typed or raw.
fn impl_ext(raw: bool) -> crate::ir::Extension {
    crate::ir::Extension {
        name: "save_note".into(),
        kind: crate::ir::ExtKind::Impl,
        signature: None,
        raw,
        bindings: [("go".to_string(), "ext/go/save.go#SaveNote".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }
}

#[test]
fn an_operation_with_neither_a_descriptor_nor_an_impl_fails_loudly() {
    // The schema fixture carries no wire_descriptor (it is the canonical
    // pre-protocol encoding) and binds no impl, a combination the emit gate
    // refuses; a direct library caller that skipped it gets a diagnosable
    // method rather than one that does not compile.
    let module = fixture_module();
    let serde = entry_text(&module);
    assert!(!serde.contains("var saveNoteDescriptor"));
    assert!(serde.contains("errors.New(\"operation has no implementation for Go\")"));
}

#[test]
fn a_typed_impl_calls_the_bound_symbol_and_guards_the_error_boundary() {
    let mut module = fixture_module();
    module.extensions = vec![impl_ext(false)];
    let serde = entry_text(&module);
    // Settings and input reach the bespoke symbol, called unqualified: the
    // bound file is dropped into the generated package.
    assert!(serde.contains("out, err := SaveNote(ctx, &c.settings, input)"));
    // A declared SDK error crosses typed; anything else is named.
    assert!(serde.contains("var known interface{ sdkError() }"));
    assert!(serde.contains("if errors.As(err, &known) {"));
    assert!(serde.contains("return zero, &ContractError{ContractName: \"save_note\", Cause: err}"));
    // The typed form needs no discrimination: declared errors arrive typed.
    assert!(!serde.contains("func DecodeSaveNoteError("));
    // The expected bespoke signature is documented above the method.
    assert!(serde
        .contains("//\tfunc SaveNote(ctx context.Context, s *Settings, input Note) (Note, error)"));
}

#[test]
fn a_raw_impl_decodes_the_outcome_and_discriminates_by_code() {
    let mut module = fixture_module();
    module.extensions = vec![impl_ext(true)];
    let serde = entry_text(&module);
    // The input travels as its wire bytes; the outcome comes back raw.
    assert!(serde.contains("payload, err := json.Marshal(input)"));
    assert!(serde.contains("outcome, err := SaveNote(ctx, &c.settings, payload)"));
    // A failing outcome discriminates on the code alone: a bespoke
    // implementation carries no protocol status.
    assert!(serde.contains("if !outcome.Success {"));
    assert!(serde.contains("return zero, DecodeSaveNoteError(outcome.Code, outcome.Body)"));
    assert!(serde.contains("func DecodeSaveNoteError(code string, body []byte) error {"));
    assert!(serde.contains("if code == \"overloaded\" {"));
    assert!(serde.contains("return &APIError{Status: 0, Body: string(body)}"));
    // The success payload decodes exactly as a protocol response does.
    assert!(serde.contains("if err := json.Unmarshal(outcome.Body, &probe); err != nil {"));
    assert!(serde
        .contains("&DecodeError{Path: \"$.id\", Expected: \"Note\", Raw: string(outcome.Body)}"));
    assert!(serde.contains(
        "//\tfunc SaveNote(ctx context.Context, s *Settings, payload []byte) (tonoext.Outcome, error)"
    ));
}

#[test]
fn the_unimplemented_op_error_passes_through_the_bound_on_error_hook() {
    let mut module = fixture_module();
    module.extensions = vec![crate::ir::Extension {
        name: "on_error".into(),
        kind: crate::ir::ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("go".to_string(), "ext/go/err.go#MapError".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }];
    let serde = entry_text(&module);
    assert!(serde.contains(
        "return zero, onErrorHook(&ContractError{ContractName: \"save_note\", Cause: errors.New(\"operation has no implementation for Go\")})"
    ));
}

#[test]
fn after_response_and_on_error_hooks_get_boundary_wrappers() {
    let mut module = fixture_module();
    let hook = |name: &str, binding: &str| crate::ir::Extension {
        name: name.into(),
        kind: crate::ir::ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("go".to_string(), binding.to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    };
    module.extensions = vec![
        hook("after_response", "ext/go/log.go#LogResponse"),
        hook("on_error", "ext/go/err.go#MapError"),
    ];
    let serde = entry_text(&module);
    assert!(serde.contains("func afterResponseHook(ctx context.Context, res tonohttp.CanonicalResponse) (tonohttp.CanonicalResponse, error) {"));
    assert!(serde.contains("func onErrorHook(err error) error {"));
    // The wrapper preserves any generated SDK error (marker interface), wrapping
    // only a foreign one as a ContractError. It no longer keeps just
    // *ContractError.
    assert!(serde.contains("var known interface{ sdkError() }"));
    assert!(serde.contains("if errors.As(err, &known) {"));
}

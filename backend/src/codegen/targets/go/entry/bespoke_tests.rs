//! The bespoke-boundary half of the Go entry tests: bound hooks, the wrappers
//! they are called through, an operation whose body is a bound `impl`, and the
//! native tests conformance vectors generate. Split from the construction and
//! resolution tests so each file stays within the repo size gate.

use std::collections::BTreeMap;

use super::tests::{entry_text, fixture_module};
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::{
    eq, impl_extension, notes_bed, push_entry_op_trait, rendered, request_pattern, wired,
    with_tests,
};
use crate::ir::{
    HttpAnswer, StubAnswer, StubDep, TestConstruction, TestDecl, TestExpect, TestPattern, TestStub,
};

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
    // The mutually exclusive transport slots are rejected at construction.
    assert!(serde.contains("if s.HTTPClient != nil && s.Transport != nil {"));
    assert!(serde.contains(
        "errors.New(\"Settings.HTTPClient and Settings.Transport are mutually exclusive: set the native slot or the canonical slot, not both\")"
    ));
    // before_request rides the client once, typed against the support shapes.
    assert!(serde.contains("func beforeRequestHook(ctx context.Context, req support.HTTPRequest) (support.HTTPRequest, error) {"));
    assert!(serde.contains("hooks: &transport.Hooks{BeforeRequest: beforeRequestHook}"));
    // The hook order lands in the emitted text: init before the requires.
    let init = serde.find("clientInitHook(&s)").unwrap();
    let require = serde
        .find("&ConfigError{Message: \"endpoint <- \"")
        .unwrap();
    assert!(init < require);
}

/// An `ext impl` binding for the fixture's `save_note`, typed or raw.
fn impl_ext(raw: bool) -> crate::ir::Extension {
    impl_extension("go", "save_note", "ext/go/save.go#SaveNote", raw)
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
    // Settings and input reach the bespoke symbol through the package-level
    // seam variable, so a generated test can swap the implementation; the
    // bound file is still dropped into the generated package.
    assert!(serde.contains("var saveNoteImpl = SaveNote"));
    assert!(serde.contains("out, err := saveNoteImpl(ctx, &c.settings, input)"));
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
    // The input travels as its wire bytes; the outcome comes back raw, through
    // the same swappable seam the typed form goes through.
    assert!(serde.contains("payload, err := json.Marshal(input)"));
    assert!(serde.contains("var saveNoteImpl = SaveNote"));
    assert!(serde.contains("outcome, err := saveNoteImpl(ctx, &c.settings, payload)"));
    // A failing outcome discriminates on the code alone: a bespoke
    // implementation carries no protocol status.
    assert!(serde.contains("if !outcome.Success {"));
    assert!(serde.contains("return zero, DecodeSaveNoteError(outcome.Code, outcome.Body)"));
    assert!(serde.contains("func DecodeSaveNoteError(code string, body []byte) error {"));
    assert!(serde.contains("if code == codeOverloaded {"));
    assert!(serde.contains("return &APIError{Status: 0, Body: string(body)}"));
    // The success payload decodes exactly as a protocol response does, through
    // the same per-type DecodeNote a protocol operation returning Note shares.
    assert!(serde.contains("out, path, ok := DecodeNote(outcome.Body)"));
    assert!(
        serde.contains("&DecodeError{Path: path, Expected: \"Note\", Raw: string(outcome.Body)}")
    );
    assert!(serde.contains(
        "//\tfunc SaveNote(ctx context.Context, s *Settings, payload []byte) (tonoext.Outcome, error)"
    ));
}

#[test]
fn declared_tests_swap_the_constructor_for_the_transport_seam_variant() {
    let mut module = with_tests(fixture_module(), notes_bed().impl_echo_tests());
    module.extensions = vec![impl_ext(false)];
    let emission = super::emit(&module, &go_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    let text = rendered(&decls, &GoRules::default());
    // The public surface is unchanged; the body moved into the unexported
    // variant the generated tests construct through.
    assert!(text.contains(
        "func New(apiKey string, opts ...ClientOption) (*Client, error) {\n\treturn newWithTransport(nil, apiKey, opts...)\n}"
    ));
    assert!(text.contains(
        "func newWithTransport(canonical support.HTTPTransport, apiKey string, opts ...ClientOption) (*Client, error) {"
    ));
    // The override lands after client_init and validation, so the test
    // transport wins over anything bespoke.
    assert!(text.contains(
        "if canonical != nil {\n\t\ts.Transport = canonical\n\t\ts.HTTPClient = nil\n\t}"
    ));
    // Without declared tests the seam is not emitted at all.
    module.tests.clear();
    let plain = entry_text(&module);
    assert!(!plain.contains("newWithTransport"));
}

#[test]
fn declared_tests_generate_a_hermetic_and_a_live_go_test_file() {
    let mut module = with_tests(fixture_module(), notes_bed().impl_echo_tests());
    module.extensions = vec![impl_ext(false)];
    let files = super::vector_tests::test_files(&module, &go_casing());
    assert_eq!(files.len(), 2);
    let hermetic = rendered(&files[0].file.decls, &GoRules::default());
    assert_eq!(files[0].group.tests_of(), Some(("client", false)));
    // The test swaps the impl seam, restores it, and runs the real method.
    assert!(hermetic.contains("func TestSaveNoteStoresIt(t *testing.T) {"));
    assert!(hermetic.contains("prev := saveNoteImpl"));
    assert!(hermetic.contains("defer func() { saveNoteImpl = prev }()"));
    assert!(hermetic.contains("c.SaveNote(context.Background(), input)"));
    // The @arg value comes from the pinned construction values; env chains not
    // covered by them are pinned absent (Setenv + Unsetenv inline, the why
    // riding the first occurrence only) so resolution is deterministic
    // anywhere.
    assert!(hermetic.contains("c, err := New(\"k\")"));
    assert!(hermetic.contains("t.Setenv(\"ENDPOINT_VERSION\", \"\")"));
    assert!(hermetic.contains("os.Unsetenv(\"ENDPOINT_VERSION\")"));
    assert_eq!(
        hermetic
            .matches("// Setenv records the restore; Unsetenv makes the variable truly absent.")
            .count(),
        1
    );
    // The wire spelling is what is compared, never the language spelling; the
    // stub's declared value decodes inline through the generated type.
    assert!(hermetic.contains("blob, err := json.Marshal(out)"));
    assert!(hermetic.contains("if err := json.Unmarshal(blob, &got); err != nil {"));
    assert!(hermetic.contains("if !reflect.DeepEqual(got, want) {"));
    assert!(hermetic
        .contains("if err := json.Unmarshal([]byte(`{\"id\":\"n1\"}`), &out); err != nil {"));
    // Zero helper functions: every test body is self-contained.
    assert!(!hermetic.contains("func vector"));
    // The live test constructs off the ambient environment, tagged out of a
    // default run by its file (the `Live` suffix keeps the names apart when a
    // tagged run compiles both files).
    let live = rendered(&files[1].file.decls, &GoRules::default());
    assert_eq!(files[1].group.tests_of(), Some(("client", true)));
    assert!(live.contains("func TestSaveNoteHitsTheRealStoreLive(t *testing.T) {"));
    assert!(!live.contains("t.Setenv"));
    assert!(!live.contains("func vector"));
}

#[test]
fn an_http_stub_generates_a_request_matching_test() {
    let bed = notes_bed();
    let module = wired(
        fixture_module(),
        vec![bed.retry_request_test(
            "/notes",
            TestPattern::Struct(
                bed.struct_pattern(true, vec![("id", eq(serde_json::json!("n1")))]),
            ),
        )],
    );
    let files = super::vector_tests::test_files(&module, &go_casing());
    assert_eq!(files.len(), 1);
    let text = rendered(&files[0].file.decls, &GoRules::default());
    // The stubbed transport records every canonical request and answers the
    // canned sequence, the last response repeating; construction goes through
    // the seam variant.
    assert!(text.contains("seen = append(seen, req)"));
    assert!(text.contains("responses := []support.HTTPResponse{{Status: 500,"));
    assert!(text.contains("if i >= len(responses) {"));
    assert!(text.contains("c, err := newWithTransport(transport, \"k\")"));
    // The open struct pattern compares field by field over the wire form.
    assert!(text.contains("var got map[string]any"));
    assert!(text.contains("if err := json.Unmarshal(blob, &got); err != nil {"));
    assert!(text.contains("!reflect.DeepEqual(got[\"id\"], want)"));
    // The whole request-pattern list matches all recorded requests with equal
    // length, in order; the path parses inline and the headers compare through
    // one lowercased copy per request.
    assert!(text.contains("if len(seen) != 2 {"));
    assert!(text.contains("req0 := seen[0]"));
    assert!(text.contains("req1 := seen[1]"));
    assert!(text.contains("if req0.Method != \"POST\" {"));
    assert!(text.contains("u0, err := url.Parse(req0.URL)"));
    assert!(text.contains("if u0.EscapedPath() != \"/notes\" {"));
    assert!(text.contains("lower1 := map[string]string{}"));
    assert!(text.contains("for k, v := range req1.Headers {"));
    assert!(text.contains("lower1[strings.ToLower(k)] = v"));
    assert!(text.contains("if lower1[\"authorization\"] != \"Bearer k\" {"));
    assert!(!text.contains("func vector"));
}

/// Two entries in one module share one Go package: their generated test files
/// must not redefine any symbol, which is why the tests are self-contained
/// (no helper functions) and multi-entry test names carry the entry prefix.
#[test]
fn two_entries_with_tests_share_the_package_without_redefining_symbols() {
    let mut module = fixture_module();
    push_entry_op_trait(
        &mut module,
        "wire_descriptor",
        serde_json::json!({"http_method": "POST", "uri": "/notes", "bindings": {}}),
    );
    // Clone the entry as a sibling so the module carries two testable entries.
    let mut admin = module
        .shapes
        .iter()
        .find(|s| s.id == "notes#client")
        .expect("fixture entry")
        .clone();
    admin.id = "notes#admin".into();
    if let crate::ir::ShapeKind::Entry { operations, .. } = &mut admin.kind {
        for op in operations {
            op.id = op.id.replace("client.", "admin.");
        }
    }
    module.shapes.push(admin);
    let test_for = |entry: &str| TestDecl {
        // The same declared name in both entries: only the entry prefix keeps
        // the generated functions apart.
        name: "hits the wire".into(),
        constructions: vec![TestConstruction {
            binding: "c".into(),
            entry: entry.into(),
            values: BTreeMap::from([("api_key".to_string(), serde_json::json!("k"))]),
        }],
        stubs: vec![TestStub {
            binding: Some("s".into()),
            client: "c".into(),
            op: "save_note".into(),
            dep: StubDep::Http,
            answers: vec![StubAnswer::Http(HttpAnswer {
                status: 200,
                headers: BTreeMap::new(),
                body: "{\"id\":\"n1\"}".into(),
            })],
        }],
        calls: vec![notes_bed().call()],
        expects: vec![
            notes_bed().echo_expect(),
            TestExpect::Requests {
                subject: "s".into(),
                requests: vec![request_pattern(
                    vec![("path", "/notes")],
                    vec![("authorization", eq(serde_json::json!("Bearer k")))],
                )],
            },
        ],
    };
    module.tests = vec![test_for("client"), test_for("admin")];
    let files = super::vector_tests::test_files(&module, &go_casing());
    assert_eq!(files.len(), 2);
    let client = rendered(&files[0].file.decls, &GoRules::default());
    let admin = rendered(&files[1].file.decls, &GoRules::default());
    // The entry prefix keeps the equally-named tests apart across the package.
    assert!(client.contains("func TestClientSaveNoteHitsTheWire(t *testing.T) {"));
    assert!(admin.contains("func TestAdminSaveNoteHitsTheWire(t *testing.T) {"));
    // A single canned answer returns directly: no slice, no index clamp.
    assert!(client.contains("return support.HTTPResponse{Status: 200,"));
    assert!(!client.contains("responses :="));
    // Zero helper functions, and no function declared by both files.
    assert!(!client.contains("func vector"));
    assert!(!admin.contains("func vector"));
    let funcs = |text: &str| -> std::collections::BTreeSet<String> {
        text.lines()
            .filter(|l| l.starts_with("func "))
            .map(str::to_string)
            .collect()
    };
    assert!(funcs(&client).is_disjoint(&funcs(&admin)));
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
    assert!(serde.contains("func afterResponseHook(ctx context.Context, res support.HTTPResponse) (support.HTTPResponse, error) {"));
    assert!(serde.contains("func onErrorHook(err error) error {"));
    // The wrapper preserves any generated SDK error (marker interface), wrapping
    // only a foreign one as a ContractError. It no longer keeps just
    // *ContractError.
    assert!(serde.contains("var known interface{ sdkError() }"));
    assert!(serde.contains("if errors.As(err, &known) {"));
}

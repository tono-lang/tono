//! The bespoke-boundary half of the Go entry tests: an operation whose body
//! is a bound `impl`, and the native tests conformance vectors generate.
//! Split from the construction and resolution tests so each file stays
//! within the repo size gate.

use std::collections::BTreeMap;

use super::tests::{entry_text, fixture_module};
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::{
    bare_entry_field, eq, impl_extension, member, notes_bed, push_entry_field, push_entry_op_wire,
    rendered, request_pattern, structure, wired, with_tests,
};
use crate::ir::{
    HttpAnswer, Prim, Source, StubAnswer, StubDep, TestConstruction, TestDecl, TestExpect,
    TestPattern, TestStub, Tref,
};

/// An `ext impl` binding for the fixture's `save_note`, typed or raw.
fn impl_ext(raw: bool) -> crate::ir::Extension {
    impl_extension("go", "save_note", "ext/go/save.go#SaveNote", raw)
}

#[test]
fn an_operation_with_neither_a_descriptor_nor_an_impl_fails_loudly() {
    // The schema fixture carries no wire binding (it is the canonical
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
fn the_constructor_is_three_steps_a_test_can_compose() {
    let mut module = with_tests(fixture_module(), notes_bed().impl_echo_tests());
    module.extensions = vec![impl_ext(false)];
    let emission = super::emit(&module, &go_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    let text = rendered(&decls, &GoRules::default());
    // The public surface is `New` alone; it runs the shared settings step
    // and the last step, which a generated test composes itself.
    assert!(text.contains(
        "func New(apiKey string, opts ...ClientOption) (*Client, error) {\n\ts, err := newSettings(apiKey, opts...)\n\tif err != nil {\n\t\treturn nil, err\n\t}\n\treturn newClient(s)\n}"
    ), "{text}");
    assert!(
        text.contains("func newSettings(apiKey string, opts ...ClientOption) (Settings, error) {")
    );
    // The last step builds the client over the assembled settings; with no
    // wire operation there is no transport to check (see the http stub test
    // for the exclusivity check that a wired entry keeps here).
    assert!(
        text.contains(
            "func newClient(s Settings) (*Client, error) {\n\treturn &Client{settings: s}, nil\n}"
        ),
        "{text}"
    );
    // No seam of any kind: nothing in production takes a transport or an
    // override for a test's sake.
    assert!(!text.contains("newWithTransport"));
    assert!(!text.contains("canonical"));
    assert!(!text.contains("Override"));
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
    // canned sequence, the last response repeating; the test assembles the
    // client itself, assigning the transport where the user would.
    assert!(text.contains("seen = append(seen, req)"));
    assert!(text.contains("responses := []support.HTTPResponse{{Status: 500,"));
    assert!(text.contains("if i >= len(responses) {"));
    assert!(text.contains(
        "\tbuild := func() (*Client, error) {\n\t\ts, err := newSettings(\"k\")\n\t\tif err != nil {\n\t\t\treturn nil, err\n\t\t}\n\t\ts.Transport = transport\n\t\treturn newClient(s)\n\t}\n\tc, err := build()\n"
    ), "{text}");
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
    push_entry_op_wire(&mut module, "POST");
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
        extern_stubs: vec![],
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

/// The fixture entry with three more `@arg` fields, each a shape whose JSON
/// spelling is not Go: a list of floats, a list of strings, and a structure
/// holding a list and an optional scalar.
fn module_with_composite_args() -> crate::ir::Module {
    let mut module = fixture_module();
    module.shapes.push(structure(
        "notes#reading",
        vec![
            member("xs", Tref::List(Box::new(Tref::Prim(Prim::I32))), true),
            member("tag", Tref::Prim(Prim::String), false),
            member(
                "again",
                Tref::Ref {
                    id: "notes#reading".into(),
                    args: vec![],
                },
                false,
            ),
        ],
    ));
    let list = |inner: Prim| Tref::List(Box::new(Tref::Prim(inner)));
    push_entry_field(
        &mut module,
        bare_entry_field("samples", list(Prim::Float), vec![Source::Arg]),
    );
    push_entry_field(
        &mut module,
        bare_entry_field("names", list(Prim::String), vec![Source::Arg]),
    );
    push_entry_field(
        &mut module,
        bare_entry_field(
            "inner",
            Tref::Ref {
                id: "notes#reading".into(),
                args: vec![],
            },
            vec![Source::Arg],
        ),
    );
    push_entry_field(
        &mut module,
        bare_entry_field(
            "weights",
            Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::I32)),
            ),
            vec![Source::Arg],
        ),
    );
    push_entry_field(
        &mut module,
        bare_entry_field("count", Tref::Prim(Prim::I32), vec![Source::Arg]),
    );
    push_entry_field(
        &mut module,
        bare_entry_field("strict", Tref::Prim(Prim::Bool), vec![Source::Arg]),
    );
    module
}

/// The hermetic test file of the fixture's echo test, its construction
/// pinning `values` on top of the bed's `api_key` (or nothing at all, when
/// `values` is empty, so every `@arg` falls back to its zero value).
fn hermetic_with_values(values: Vec<(&str, serde_json::Value)>) -> String {
    let bed = notes_bed();
    let mut tests = bed.impl_echo_tests();
    tests.truncate(1);
    if values.is_empty() {
        tests[0].constructions[0].values.clear();
    }
    for (key, value) in values {
        tests[0].constructions[0]
            .values
            .insert(key.to_string(), value);
    }
    let mut module = with_tests(module_with_composite_args(), tests);
    module.extensions = vec![impl_ext(false)];
    let files = super::vector_tests::test_files(&module, &go_casing());
    rendered(&files[0].file.decls, &GoRules::default())
}

#[test]
fn a_pinned_list_or_structure_is_a_typed_composite_literal() {
    let hermetic = hermetic_with_values(vec![
        ("samples", serde_json::json!([1.0, 2.0, 3.0])),
        ("names", serde_json::json!(["x", "y"])),
        ("inner", serde_json::json!({"xs": [1, 2], "tag": "t"})),
    ]);
    // A list is `[]T{..}` typed by the parameter, a structure is `T{..}`
    // naming its members by their Go field names; the optional scalar member
    // is a pointer field, bound through a closure since a literal has no
    // address.
    assert!(
        hermetic.contains(
            "c, err := New(\"k\", []float64{float64(1.0), float64(2.0), float64(3.0)}, []string{\"x\", \"y\"}, Reading{Xs: []int32{int32(1), int32(2)}, Tag: func() *string { v := \"t\"; return &v }()}, nil, 0, false)"
        ),
        "{hermetic}"
    );
}

#[test]
fn an_empty_list_is_an_empty_composite_literal_and_an_unpinned_one_is_nil() {
    let hermetic = hermetic_with_values(vec![
        ("samples", serde_json::json!([])),
        ("inner", serde_json::json!({"xs": [], "again": {"xs": [3]}})),
        ("weights", serde_json::json!({"a": 1, "b": 2})),
        ("count", serde_json::json!(7)),
        ("strict", serde_json::json!(true)),
    ]);
    // `names` is left unpinned: the zero value of a slice is nil. A map is
    // its own composite literal; an optional structure member is addressable
    // as a literal, so it takes a plain `&`.
    assert!(
        hermetic.contains(
            "c, err := New(\"k\", []float64{}, nil, Reading{Xs: []int32{}, Again: &Reading{Xs: []int32{int32(3)}}}, map[string]int32{\"a\": int32(1), \"b\": int32(2)}, int32(7), true)"
        ),
        "{hermetic}"
    );
}

#[test]
fn every_unpinned_argument_is_its_type_zero_value() {
    let hermetic = hermetic_with_values(vec![]);
    assert!(
        hermetic.contains("c, err := New(\"\", nil, nil, Reading{}, nil, 0, false)"),
        "{hermetic}"
    );
}

#[test]
fn an_unpinned_structure_argument_is_its_empty_literal() {
    let hermetic = hermetic_with_values(vec![("names", serde_json::json!(["only"]))]);
    assert!(
        hermetic
            .contains("c, err := New(\"k\", nil, []string{\"only\"}, Reading{}, nil, 0, false)"),
        "{hermetic}"
    );
}

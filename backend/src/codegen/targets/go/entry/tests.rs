use super::*;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::rendered;
use crate::ir::decode_model;

/// The canonical entry fixture (config, every source kind, derivation,
/// selection, composition, protocol refs), decoded off the shared schema
/// so the emitter is exercised against the real wire shape.
fn fixture_module() -> Module {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ir-schema/fixtures/entries_client.json"
    ));
    let model = decode_model(text).expect("fixture decodes");
    model.modules.into_iter().next().expect("one module")
}

fn types_text(module: &Module) -> String {
    rendered(&type_decls(module, &go_casing()), &GoRules::default())
}

fn serde_text(module: &Module) -> String {
    rendered(&serde_decls(module, &go_casing()), &GoRules::default())
}

#[test]
fn the_construction_surface_is_new_options_settings_and_the_mock_interface() {
    let module = fixture_module();
    let types = types_text(&module);
    // Settings carry every resolved field plus the transport slots.
    assert!(types.contains("type Settings struct {"));
    assert!(types.contains("\tHTTPClient *http.Client\n"));
    assert!(types.contains("\tTransport  tonohttp.Transport\n"));
    assert!(types.contains("\tHeaders    map[string]string\n"));
    // The config is a construction-only struct.
    assert!(types.contains("type Conf struct {"));
    // One functional option per @with field, unprefixed (single entry).
    assert!(types.contains("func WithClientName(v string) ClientOption {"));
    assert!(types.contains("func WithTimeout(v Duration) ClientOption {"));
    assert!(types.contains("func WithMaxRetries(v int32) ClientOption {"));
    // The mock interface has ctx first and the conformance assertion.
    assert!(types.contains("type ClientAPI interface {"));
    assert!(types.contains("SaveNote(ctx context.Context, input Note) (Note, error)"));
    assert!(types.contains("var _ ClientAPI = (*Client)(nil)"));

    let serde = serde_text(&module);
    assert!(serde.contains("func New(apiKey string, opts ...ClientOption) (*Client, error) {"));
}

#[test]
fn the_resolution_follows_the_declared_chains() {
    let module = fixture_module();
    let serde = serde_text(&module);
    // @arg lands positionally; @with falls back to @default.
    assert!(serde.contains("s.APIKey = apiKey"));
    assert!(serde.contains("if w.clientName != nil {"));
    assert!(serde.contains("s.ClientName = \"demo\""));
    // @format with @str transforms.
    assert!(serde.contains("s.ClientKey = strUpperSnake(strings.TrimSpace(s.ClientName))"));
    assert!(serde.contains("s.EndpointEnv = \"ENDPOINT_\" + s.ClientKey + \"_V2\""));
    // A dynamic env name reads through the resolved field.
    assert!(serde.contains("os.LookupEnv(s.EndpointEnv)"));
    // The match lowers to a switch with the wildcard as default.
    assert!(serde.contains("switch s.EndpointVersion {"));
    assert!(serde.contains("case \"v1\":"));
    assert!(serde.contains("case \"legacy\":"));
    assert!(serde.contains("s.Endpoint = \"https://old.example.com\""));
    // An arm that reads an absent chain reports it at the point of use.
    assert!(serde.contains("endpointWhy = \"endpoint_v1 <- \" + endpointV1Why"));
    // @bind: the entry value feeds the composed member; the unbound member
    // keeps its own chain.
    assert!(serde.contains("composed.APIKey = s.APIKey"));
    assert!(serde.contains("composed.Region = \"us\""));
    // The resolved values freeze for the runtime's ref positions, ints
    // widened and durations in milliseconds.
    assert!(serde.contains("values[\"max_retries\"] = int64(s.MaxRetries)"));
    assert!(serde.contains("ms, err := durationMs(string(s.Timeout))"));
    assert!(serde.contains("values[\"settings.api_key\"] = s.Settings.APIKey"));
}

#[test]
fn the_env_boundary_parses_by_type_naming_variable_and_type() {
    let mut module = fixture_module();
    // Give a field an env-sourced integer to exercise the parse path.
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(EntryField {
                name: "port".into(),
                target: Tref::Prim(Prim::I32),
                sources: vec![Source::Env(EnvName::Name("PORT".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let serde = serde_text(&module);
    assert!(serde.contains("strconv.ParseInt(v, 10, 32)"));
    assert!(serde.contains("fmt.Errorf(\"%s: invalid i32 %q\", \"PORT\", v)"));
}

#[test]
fn a_multi_entry_module_prefixes_the_colliding_companions() {
    let mut module = fixture_module();
    let second = {
        let mut clone = module
            .shapes
            .iter()
            .find(|s| matches!(s.kind, ShapeKind::Entry { .. }))
            .cloned()
            .expect("entry");
        clone.id = "notes#admin".into();
        clone
    };
    module.shapes.push(second);
    let types = types_text(&module);
    assert!(types.contains("type ClientSettings struct {"));
    assert!(types.contains("type AdminSettings struct {"));
    assert!(types.contains("func WithClientClientName(v string) ClientOption {"));
    assert!(types.contains("func WithAdminClientName(v string) AdminOption {"));
    let serde = serde_text(&module);
    assert!(
        serde.contains("func NewClient(apiKey string, opts ...ClientOption) (*Client, error) {")
    );
    assert!(serde.contains("func NewAdmin(apiKey string, opts ...AdminOption) (*Admin, error) {"));
}

#[test]
fn bound_hooks_wire_the_settings_bridge_and_the_transport_slots() {
    let mut module = fixture_module();
    module.extensions = vec![
        crate::ir::Extension {
            name: "client_init".into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            bindings: [("go".to_string(), "ext/go/init.go#InitSettings".to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        },
        crate::ir::Extension {
            name: "before_request".into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            bindings: [("go".to_string(), "ext/go/auth.go#AddBearer".to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        },
    ];
    let serde = serde_text(&module);
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
    let require = serde.find("errors.New(\"endpoint <- \"").unwrap();
    assert!(init < require);
}

/// Attach an opaque descriptor to every entry op, standing in for the
/// frontend's protocol pass (the schema fixture is pre-protocol).
fn with_descriptors(mut module: Module) -> Module {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                op.traits.push(crate::ir::Trait {
                    id: "wire_descriptor".into(),
                    value: serde_json::json!({"http_method": "POST", "uri": "/notes/{id}"}),
                });
            }
        }
    }
    module
}

#[test]
fn the_method_maps_the_raw_outcome_onto_the_taxonomy() {
    let module = with_descriptors(fixture_module());
    let serde = serde_text(&module);
    // The descriptor is embedded verbatim, an opaque blob.
    assert!(serde.contains("var saveNoteDescriptor = mustDescriptor("));
    assert!(serde
        .contains("outcome, err := c.runtime.Execute(ctx, saveNoteDescriptor, record, c.hooks)"));
    assert!(serde.contains("case tonohttp.OutcomeTransport:"));
    assert!(serde.contains("&TransportError{Cause: outcome.Cause}"));
    assert!(serde.contains("DecodeSaveNoteError(outcome.Status, []byte(outcome.Body))"));
    assert!(serde.contains("&DecodeError{Path: \"$\", Expected: \"Note\", Raw: outcome.Body}"));
}

#[test]
fn a_dynamic_env_name_off_an_absent_chain_emits_balanced_braces() {
    // A non-guaranteed chain whose env name comes from a sibling that may
    // itself be absent: the emitted run must be one balanced
    // if/else-if/else (the else chains straight into the lookup).
    let mut module = fixture_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            // naming is env-only (not guaranteed); reader looks its value up.
            fields.push(EntryField {
                name: "naming".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![Source::Env(EnvName::Name("NAMING".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(EntryField {
                name: "reader".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![Source::Env(EnvName::Field(crate::ir::FieldRef {
                    field: vec!["naming".into()],
                }))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let serde = serde_text(&module);
    assert!(serde.contains("readerWhy = \"naming <- \" + namingWhy"));
    let new_fn = serde
        .split("func New(")
        .nth(1)
        .and_then(|rest| rest.split("\nfunc ").next())
        .expect("New body");
    assert_eq!(
        new_fn.matches('{').count(),
        new_fn.matches('}').count(),
        "unbalanced braces in the generated constructor:\n{new_fn}"
    );
}

#[test]
fn structured_sources_decode_strictly_and_honor_explicit_values() {
    let mut module = fixture_module();
    let creds = Shape {
        id: "notes#credentials".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![
                Member {
                    name: "token".into(),
                    target: Tref::Prim(Prim::String),
                    required: true,
                    default: None,
                    constraints: vec![crate::ir::Constraint::Length {
                        min: Some(1),
                        max: None,
                    }],
                    traits: vec![],
                },
                Member {
                    name: "account_id".into(),
                    target: Tref::Prim(Prim::String),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                },
            ],
        },
        traits: vec![],
    };
    module.shapes.push(creds);
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            // @with layers over the env decode; a labels map decodes whole.
            fields.push(EntryField {
                name: "creds".into(),
                target: Tref::Ref {
                    id: "notes#credentials".into(),
                    args: vec![],
                },
                sources: vec![
                    Source::With,
                    Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into())),
                ],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(EntryField {
                name: "labels".into(),
                target: Tref::Map(
                    Box::new(Tref::Prim(Prim::String)),
                    Box::new(Tref::Prim(Prim::String)),
                ),
                sources: vec![Source::Env(EnvName::Name("SERVICE_LABELS".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let serde = serde_text(&module);
    // Strict decode: probe for required members, unknown fields rejected,
    // declared validation at construction, the env name as context.
    assert!(serde.contains("fmt.Errorf(\"%s: missing field token\", \"SERVICE_CREDENTIALS\")"));
    assert!(serde.contains("dec.DisallowUnknownFields()"));
    assert!(serde.contains("ValidateCredentials(decoded)"));
    // The explicit @with value wins: the decode runs only while unset.
    assert!(serde.contains("if w.creds != nil {"));
    assert!(serde.contains("if credsWhy != \"\" {"));
    // The whole-JSON map field decodes with its env name as context.
    assert!(serde.contains("json.Unmarshal([]byte(raw), &s.Labels)"));
    let types = types_text(&module);
    assert!(types.contains("func WithCreds(v Credentials) ClientOption {"));
}

#[test]
fn a_total_select_without_wildcard_fails_construction_on_an_open_enum_value() {
    let mut module = fixture_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            let mut choice = EntryField {
                name: "choice".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            };
            choice.select = Some(crate::ir::Select {
                subject: vec!["client_name".into()],
                arms: vec![crate::ir::SelectArm {
                    pattern: Some(serde_json::json!("demo")),
                    value: crate::ir::ArmValue::Lit(serde_json::json!("d")),
                }],
            });
            fields.push(choice);
        }
    }
    let serde = serde_text(&module);
    assert!(serde.contains(
            "return nil, fmt.Errorf(\"choice: match on client_name: unmatched value %v\", s.ClientName)"
        ));
}

#[test]
fn an_operation_without_a_descriptor_stubs_with_a_contract_error() {
    // The schema fixture carries no wire_descriptor (it is the canonical
    // pre-protocol encoding), so its op method must be the bespoke stub.
    let module = fixture_module();
    let serde = serde_text(&module);
    assert!(!serde.contains("var saveNoteDescriptor"));
    assert!(serde.contains("errors.New(\"operation has no transport binding\")"));
}

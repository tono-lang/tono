use super::*;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::{
    bare_entry_field, push_config_member, push_entry_field, push_entry_op_trait, rendered,
    set_entry_op_outputs, with_bytes_and_constrained_port, with_derived_config_members,
    with_enum_config_member, with_member_select_on_absent_subject, with_structured_sources,
    with_transformed_chain_field,
};
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

/// Everything a module's entries emit: the declarations they share (which ride
/// the module's internal group) followed by each entry's own group.
fn entry_text(module: &Module) -> String {
    let emission = emit(module, &go_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &GoRules::default())
}

#[test]
fn the_construction_surface_is_new_options_settings_and_the_mock_interface() {
    let module = fixture_module();
    let types = entry_text(&module);
    // Settings carry every resolved field plus the transport slots.
    assert!(types.contains("type Settings struct {"));
    assert!(types.contains("\tHTTPClient *http.Client\n"));
    assert!(types.contains("\tTransport  tonohttp.Transport\n"));
    assert!(types.contains("\tHeaders    map[string]string\n"));
    // The config is a construction-only struct, hidden (unexported) from the
    // package's public surface.
    assert!(types.contains("type conf struct {"));
    assert!(!types.contains("type Conf struct {"));
    // One functional option per @with field, unprefixed (single entry).
    assert!(types.contains("func WithClientName(v string) ClientOption {"));
    assert!(types.contains("func WithTimeout(v support.Duration) ClientOption {"));
    assert!(types.contains("func WithMaxRetries(v int32) ClientOption {"));
    // The mock interface has ctx first and the conformance assertion.
    assert!(types.contains("type ClientAPI interface {"));
    assert!(types.contains("SaveNote(ctx context.Context, input Note) (Note, error)"));
    assert!(types.contains("var _ ClientAPI = (*Client)(nil)"));

    let serde = entry_text(&module);
    assert!(serde.contains("func New(apiKey string, opts ...ClientOption) (*Client, error) {"));
}

#[test]
fn the_resolution_follows_the_declared_chains() {
    let module = fixture_module();
    let serde = entry_text(&module);
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
    push_entry_field(
        &mut module,
        bare_entry_field(
            "port",
            Tref::Prim(Prim::I32),
            vec![Source::Env(EnvName::Name("PORT".into()))],
        ),
    );
    let serde = entry_text(&module);
    assert!(serde.contains("strconv.ParseInt(v, 10, 32)"));
    assert!(
        serde.contains("&ConfigError{Message: fmt.Sprintf(\"%s: invalid i32 %q\", \"PORT\", v)}")
    );
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
    let types = entry_text(&module);
    assert!(types.contains("type ClientSettings struct {"));
    assert!(types.contains("type AdminSettings struct {"));
    assert!(types.contains("func WithClientClientName(v string) ClientOption {"));
    assert!(types.contains("func WithAdminClientName(v string) AdminOption {"));
    let serde = entry_text(&module);
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
    let serde = entry_text(&module);
    // The descriptor is embedded verbatim, an opaque blob.
    assert!(serde.contains("var saveNoteDescriptor = tono.MustDescriptor("));
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
    // naming is env-only (not guaranteed); reader looks its value up.
    push_entry_field(
        &mut module,
        bare_entry_field(
            "naming",
            Tref::Prim(Prim::String),
            vec![Source::Env(EnvName::Name("NAMING".into()))],
        ),
    );
    push_entry_field(
        &mut module,
        bare_entry_field(
            "reader",
            Tref::Prim(Prim::String),
            vec![Source::Env(EnvName::Field(crate::ir::FieldRef {
                field: vec!["naming".into()],
            }))],
        ),
    );
    let serde = entry_text(&module);
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
    // @with layers over the env decode; a labels map decodes whole.
    with_structured_sources(
        &mut module,
        vec![
            Source::With,
            Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into())),
        ],
    );
    let serde = entry_text(&module);
    // Strict decode: probe for required members, unknown fields rejected,
    // declared validation at construction, the env name as context.
    assert!(serde.contains(
        "&ConfigError{Message: fmt.Sprintf(\"%s: missing field token\", \"SERVICE_CREDENTIALS\")}"
    ));
    // An explicit null in a required member is as absent as a missing key
    // (the TypeScript decode rejects it too).
    assert!(serde.contains("if rv, ok := probe[\"token\"]; !ok || string(rv) == \"null\" {"));
    assert!(serde.contains("dec.DisallowUnknownFields()"));
    assert!(serde.contains("ValidateCredentials(decoded)"));
    // The explicit @with value wins: the decode runs only while unset.
    assert!(serde.contains("if w.creds != nil {"));
    assert!(serde.contains("if credsWhy != \"\" {"));
    // The whole-JSON map field decodes with its env name as context.
    assert!(serde.contains("json.Unmarshal([]byte(raw), &s.Labels)"));
    let types = entry_text(&module);
    assert!(types.contains("func WithCreds(v Credentials) ClientOption {"));
}

#[test]
fn a_structured_source_falls_back_across_multiple_envs() {
    let mut module = fixture_module();
    // Two @env sources: the second is a fallback tried only while the first is
    // still absent (a first-present-wins cascade, not first-only).
    with_structured_sources(
        &mut module,
        vec![
            Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into())),
            Source::Env(EnvName::Name("SERVICE_CREDENTIALS_FALLBACK".into())),
        ],
    );
    let serde = entry_text(&module);
    // Both variables are read (the fallback is not dropped).
    assert!(serde.contains("os.LookupEnv(\"SERVICE_CREDENTIALS\")"));
    assert!(serde.contains("os.LookupEnv(\"SERVICE_CREDENTIALS_FALLBACK\")"));
    // The fallback runs only while the first source stayed unresolved.
    let fallback = serde
        .find("os.LookupEnv(\"SERVICE_CREDENTIALS_FALLBACK\")")
        .expect("fallback lookup");
    let guard = serde[..fallback]
        .rfind("if credsWhy != \"\" {")
        .expect("fallback guard");
    let primary = serde
        .find("os.LookupEnv(\"SERVICE_CREDENTIALS\")")
        .expect("primary lookup");
    // The guard sits between the primary decode and the fallback decode.
    assert!(primary < guard && guard < fallback);
}

#[test]
fn a_structured_decode_probes_the_wire_key_not_the_member_name() {
    let mut module = fixture_module();
    module.shapes.push(crate::ir::Shape {
        id: "notes#creds".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![crate::ir::Member {
                name: "token".into(),
                target: Tref::Prim(Prim::String),
                required: true,
                default: None,
                constraints: vec![],
                // @wire renames the serialized key; the decode must probe "tok".
                traits: vec![crate::ir::Trait {
                    id: "wire".into(),
                    value: serde_json::json!("tok"),
                }],
            }],
        },
        traits: vec![],
    });
    push_entry_field(
        &mut module,
        bare_entry_field(
            "creds",
            Tref::Ref {
                id: "notes#creds".into(),
                args: vec![],
            },
            vec![Source::Env(EnvName::Name("CREDS".into()))],
        ),
    );
    let serde = entry_text(&module);
    // The required-field probe reads the wire key, matching what the codec emits.
    assert!(serde.contains("if rv, ok := probe[\"tok\"]; !ok || string(rv) == \"null\" {"));
    assert!(serde.contains("missing field tok"));
    assert!(!serde.contains("probe[\"token\"]"));
}

#[test]
fn field_docs_flow_onto_the_settings_field_and_the_with_option() {
    // client_name carries @doc in the fixture; it must land on both its public
    // surfaces (the Settings field and the With option).
    let module = fixture_module();
    let types = entry_text(&module);
    assert!(types.contains("\t// Names the caller.\n\tClientName string"));
    // godoc needs the identifier first, so the canonical sentence leads and the
    // @doc line follows as continuation.
    assert!(types.contains(
        "// WithClientName sets the client_name construction value.\n// Names the caller."
    ));
}

#[test]
fn an_entry_field_rename_retargets_every_go_identifier() {
    let mut module = fixture_module();
    // A renamed @arg: @rename(go) retargets the public identifier and every
    // internal reference consistently, while the canonical value key is untouched.
    let mut token = bare_entry_field("primary_key", Tref::Prim(Prim::String), vec![Source::Arg]);
    // @rename(lang) is a verbatim identifier (it bypasses casing), used at every
    // position the field appears in.
    token.traits = vec![crate::ir::Trait {
        id: "rename".into(),
        value: serde_json::json!({"go": "AuthToken"}),
    }];
    push_entry_field(&mut module, token);
    let types = entry_text(&module);
    let serde = entry_text(&module);
    // The Settings field, the @arg param, and the internal write all use the
    // renamed identifier; none use the casing of the canonical name.
    assert!(types.contains("\tAuthToken string\n"));
    assert!(!types.contains("PrimaryKey"));
    assert!(serde.contains("AuthToken string"));
    assert!(serde.contains("s.AuthToken = AuthToken"));
    assert!(!serde.contains("PrimaryKey"));
    // The canonical dotted key the runtime reads is unchanged (not renamed).
    assert!(serde.contains("values[\"primary_key\"]"));
}

#[test]
fn a_total_select_without_wildcard_fails_construction_on_an_open_enum_value() {
    let mut module = fixture_module();
    let mut choice = bare_entry_field("choice", Tref::Prim(Prim::String), vec![]);
    choice.select = Some(crate::ir::Select {
        subject: vec!["client_name".into()],
        arms: vec![crate::ir::SelectArm {
            pattern: Some(serde_json::json!("demo")),
            value: crate::ir::ArmValue::Lit(serde_json::json!("d")),
        }],
    });
    push_entry_field(&mut module, choice);
    let serde = entry_text(&module);
    assert!(serde.contains(
            "return nil, &ConfigError{Message: fmt.Sprintf(\"choice: match on client_name: unmatched value %v\", s.ClientName)}"
        ));
}

#[test]
fn a_why_reason_nothing_reads_is_discarded_rather_than_left_unused() {
    // A chain no other field, trait, or bind consumes still records why it came
    // up empty, but nothing reads it back, and Go rejects a variable that is
    // assigned and never read.
    let mut module = fixture_module();
    let env = crate::ir::Source::Env(crate::ir::EnvName::Name("SPARE_TOKEN".into()));
    let field = bare_entry_field("spare_token", Tref::Prim(Prim::String), vec![env]);
    push_entry_field(&mut module, field);
    let serde = entry_text(&module);
    assert!(serde.contains("spareTokenWhy := \"no source\""));
    assert!(serde.contains("\t_ = spareTokenWhy\n"));
    // A consumed chain is read by its own check, so it is left alone.
    assert!(!serde.contains("_ = endpointVersionWhy"));
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
fn transforms_apply_to_chain_and_match_resolved_values() {
    let mut module = fixture_module();
    with_transformed_chain_field(&mut module);
    let mut picked = bare_entry_field("picked", Tref::Prim(Prim::String), vec![]);
    picked.transforms = vec!["upper".into()];
    picked.select = Some(crate::ir::Select {
        subject: vec!["client_name".into()],
        arms: vec![
            crate::ir::SelectArm {
                pattern: Some(serde_json::json!("demo")),
                value: crate::ir::ArmValue::Lit(serde_json::json!("d")),
            },
            crate::ir::SelectArm {
                pattern: None,
                value: crate::ir::ArmValue::Lit(serde_json::json!("x")),
            },
        ],
    });
    push_entry_field(&mut module, picked);
    let serde = entry_text(&module);
    // The pipeline runs over the resolved value whatever idiom produced it.
    assert!(serde.contains("s.Team = strSnake(s.Team)"));
    assert!(serde.contains("s.Picked = strings.ToUpper(s.Picked)"));
}

#[test]
fn a_config_member_keeps_its_declared_derivation() {
    let mut module = fixture_module();
    with_derived_config_members(&mut module);
    let serde = entry_text(&module);
    // The member's @format (with its transforms) lands inside the composition.
    assert!(serde.contains("composed.Label = strings.ToUpper(\"conf-\" + s.ClientName)"));
    // The member's match lowers to a switch writing the composed member; an
    // unmatched value leaves the zero (no member-level why to track).
    assert!(serde.contains("case \"demo\":"));
    assert!(serde.contains("composed.Size = \"small\""));
}

#[test]
fn the_env_boundary_decodes_bytes_and_rejects_non_decimal_floats() {
    let mut module = fixture_module();
    with_bytes_and_constrained_port(&mut module);
    push_entry_field(
        &mut module,
        bare_entry_field(
            "rate",
            Tref::Prim(Prim::Float),
            vec![Source::Env(EnvName::Name("RATE".into()))],
        ),
    );
    let serde = entry_text(&module);
    // Bytes ride the env boundary as base64, the same encoding the wire uses.
    assert!(serde.contains("base64.StdEncoding.DecodeString(v)"));
    assert!(serde
        .contains("&ConfigError{Message: fmt.Sprintf(\"%s: invalid base64 %q\", \"SECRET\", v)}"));
    // Floats take decimal notation only (no Inf/NaN/hex), like the TS boundary.
    assert!(serde.contains("strings.ContainsRune(\"0123456789+-.eE\", r)"));
}

#[test]
fn a_consumed_config_member_requires_a_value_at_construction() {
    let mut module = fixture_module();
    push_entry_op_trait(
        &mut module,
        "header",
        serde_json::json!(["X-Key", {"field": ["settings", "api_key"]}]),
    );
    let serde = entry_text(&module);
    // The leaf value itself is checked (there is no member-level why).
    assert!(serde.contains("if s.Settings.APIKey == \"\" {"));
    assert!(serde.contains("&ConfigError{Message: \"settings.api_key: no value\"}"));
}

#[test]
fn a_consumed_numeric_config_member_requires_its_resolution_not_its_zero() {
    let mut module = fixture_module();
    // A numeric config member fed only by an env: a resolved 0 is a value, so
    // absence cannot be read off the zero. It carries a hoisted reason var and
    // the consumed require reads that, not the value.
    push_config_member(
        &mut module,
        bare_entry_field(
            "max_conns",
            Tref::Prim(Prim::I32),
            vec![Source::Env(EnvName::Name("MAX_CONNS".into()))],
        ),
    );
    push_entry_op_trait(
        &mut module,
        "header",
        serde_json::json!(["X-Max", {"field": ["settings", "max_conns"]}]),
    );
    let serde = entry_text(&module);
    // The reason var is hoisted above the config block (so the post-construction
    // require can read it) and the member resolves through the tracked chain.
    assert!(serde.contains("settingsMaxConnsWhy := \"no source\""));
    // The require reads the reason, never the (possibly legitimately zero) value.
    assert!(serde.contains("if settingsMaxConnsWhy != \"\" {"));
    assert!(
        serde.contains("&ConfigError{Message: \"settings.max_conns <- \" + settingsMaxConnsWhy}")
    );
    // It is not compared against the numeric zero (that would reject a real 0).
    assert!(!serde.contains("s.Settings.MaxConns == 0"));
}

#[test]
fn a_constrained_op_input_is_validated_before_transport() {
    let mut module = with_descriptors(fixture_module());
    // Give the op input struct a constraint so it gains a validator.
    for shape in &mut module.shapes {
        if shape.id == "notes#note" {
            if let ShapeKind::Structure { members, .. } = &mut shape.kind {
                if let Some(m) = members.iter_mut().find(|m| m.name == "body") {
                    m.constraints = vec![crate::ir::Constraint::Length {
                        min: Some(1),
                        max: None,
                    }];
                }
            }
        }
    }
    let serde = entry_text(&module);
    // The input is validated and a violation surfaces as the Validation category
    // the validator itself returns.
    assert!(serde.contains("if invalid := ValidateNote(input); invalid != nil {"));
    assert!(serde.contains("return zero, invalid"));
    // The check runs before the transport call, not after.
    let val = serde.find("ValidateNote(input)").expect("validate call");
    let exec = serde
        .find("c.runtime.Execute(ctx, saveNoteDescriptor")
        .expect("execute call");
    assert!(val < exec);
}

#[test]
fn a_structured_output_decodes_strictly_on_required_members() {
    let module = with_descriptors(fixture_module());
    let serde = entry_text(&module);
    // The 2xx output is probed for its required members (a zero value is not a
    // present value) and a missing one surfaces a DecodeError, not a zeroed
    // struct. The probe reads the wire key.
    assert!(serde.contains("var probe map[string]json.RawMessage"));
    assert!(serde.contains("if rv, ok := probe[\"id\"]; !ok || string(rv) == \"null\" {"));
    assert!(serde.contains("if rv, ok := probe[\"body\"]; !ok || string(rv) == \"null\" {"));
    // A missing required member points at that member, not the whole body.
    assert!(serde.contains("&DecodeError{Path: \"$.id\", Expected: \"Note\", Raw: outcome.Body}"));
    assert!(serde.contains("&DecodeError{Path: \"$.body\", Expected: \"Note\", Raw: outcome.Body}"));
    // A whole-body parse failure still points at the root.
    assert!(serde.contains("&DecodeError{Path: \"$\", Expected: \"Note\", Raw: outcome.Body}"));
    // Unknown fields are still tolerated (a plain Unmarshal into the struct).
    assert!(serde.contains("var out Note"));
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
fn the_matrix_module_exercises_every_resolution_idiom() {
    let module = crate::codegen::test_support::entries_matrix_module();
    let types = entry_text(&module);
    let serde = entry_text(&module);
    // Typed boundaries: every env-parsed primitive spells its own parse.
    assert!(serde.contains("strconv.ParseInt(v, 10, 8)"));
    assert!(serde.contains("strconv.ParseInt(v, 10, 16)"));
    assert!(serde.contains("strconv.ParseInt(v, 10, 64)"));
    assert!(serde.contains("strconv.ParseUint(v, 10, 8)"));
    assert!(serde.contains("strconv.ParseUint(v, 10, 64)"));
    assert!(serde.contains("strconv.ParseFloat(v, 64)"));
    assert!(serde.contains("time.ParseDuration(v)"));
    assert!(serde.contains("case \"true\", \"1\":"));
    // An enum field is a branded string: cast at the boundary, frozen into
    // the values.
    assert!(serde.contains("s.Mode = Mode(v)"));
    assert!(serde.contains("values[\"mode\"] = string(s.Mode)"));
    // Guaranteed and why-tracked dynamic env names both spell one balanced run.
    assert!(serde.contains("os.LookupEnv(s.SureName)"));
    assert!(serde.contains("dynamicWhy = \"naming <- \" + namingWhy"));
    // Transforms compose innermost-first; the input placeholder renders empty.
    assert!(serde.contains("strUpperSnake(strPascal(strKebab(strSnake(strings.ToUpper(strings.ToLower(strings.TrimSpace("));
    // Both select flavors: why-tracked with an inline source arm, and a
    // guaranteed one that fails construction on an undeclared value.
    assert!(serde.contains("case 1:"));
    assert!(serde.contains("pickedWhy = \"no source\""));
    assert!(serde.contains(
        "return nil, &ConfigError{Message: fmt.Sprintf(\"sure_pick: match on sure_name: unmatched value %v\", s.SureName)}"
    ));
    // Composition: binds layered over member chains, an int member parsing.
    assert!(serde.contains("composed.Key = s.Naming"));
    assert!(serde.contains("composed.Sure = s.SureName"));
    assert!(serde.contains("strconv.ParseInt(v, 10, 32)"));
    // Structured and whole-JSON sources honor explicit values.
    assert!(serde.contains("if w.creds != nil {"));
    assert!(serde.contains("s.CredsArg = credsArg"));
    assert!(serde.contains("if w.labels != nil {"));
    assert!(serde.contains("s.Tags = tags"));
    // The four method shapes: full descriptor, bare, primitive output, stub.
    assert!(
        serde.contains("func (c *API) FetchNote(ctx context.Context, input Note) (Note, error) {")
    );
    assert!(serde.contains("func (c *API) Ping(ctx context.Context) error {"));
    assert!(serde.contains("func (c *API) Count(ctx context.Context) (int32, error) {"));
    assert!(
        serde.contains("var zero string\n\treturn zero, &ContractError{ContractName: \"local\"")
    );
    // The entry doc rides the client type.
    assert!(types.contains("// The matrix entry."));
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
fn a_config_member_match_tracks_absent_subjects_and_inline_sources() {
    let mut module = fixture_module();
    with_member_select_on_absent_subject(&mut module);
    let serde = entry_text(&module);
    // The member's switch only runs once the why-tracked subject resolved, an
    // arm reading an absent chain assigns only when that chain resolved, and
    // an inline source arm keeps the presence-only member spelling.
    assert!(serde.contains("if endpointWhy == \"\" {"));
    assert!(serde.contains("if endpointV1Why == \"\" {"));
    assert!(serde.contains("os.LookupEnv(\"ZONE\")"));
}

#[test]
fn a_consumed_bytes_head_requires_a_value_and_numeric_constraints_gate_on_presence() {
    let mut module = fixture_module();
    with_bytes_and_constrained_port(&mut module);
    let serde = entry_text(&module);
    assert!(serde.contains("if len(s.Secret) == 0 {"));
    // The numeric constraint skips when the chain reported absent and the
    // bridge left the zero in place (same presence rule as the requires).
    assert!(serde.contains("(portWhy == \"\" || s.Port != 0) &&"));
}

#[test]
fn a_64_bit_operation_output_decodes_from_its_wire_string() {
    let mut module = with_descriptors(fixture_module());
    set_entry_op_outputs(&mut module, Tref::Prim(Prim::I64));
    let serde = entry_text(&module);
    assert!(serde.contains("var wire string"));
    assert!(serde.contains("strconv.ParseInt(wire, 10, 64)"));
}

#[test]
fn an_enum_member_of_a_config_freezes_as_a_branded_string() {
    let mut module = fixture_module();
    with_enum_config_member(&mut module);
    let serde = entry_text(&module);
    assert!(serde.contains("values[\"settings.mode\"] = string(s.Settings.Mode)"));
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

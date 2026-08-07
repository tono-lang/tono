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
use crate::ir::WireValue;

/// The canonical entry fixture (config, every source kind, derivation,
/// selection, composition, protocol refs), decoded off the shared schema
/// so the emitter is exercised against the real wire shape.
pub(super) fn fixture_module() -> Module {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ir-schema/fixtures/entries_client.json"
    ));
    let model = decode_model(text).expect("fixture decodes");
    model.modules.into_iter().next().expect("one module")
}

/// Everything a module's entries emit: the declarations they share (which ride
/// the module's internal group) followed by each entry's own group.
pub(super) fn entry_text(module: &Module) -> String {
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
    assert!(types.contains("\tTransport  support.HTTPTransport\n"));
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
    assert!(serde.contains("s.ClientKey = casing.StrUpperSnake(strings.TrimSpace(s.ClientName))"));
    assert!(serde.contains("s.EndpointEnv = \"ENDPOINT_\" + s.ClientKey + \"_V2\""));
    // A dynamic env name reads through the resolved field.
    assert!(serde.contains("os.LookupEnv(s.EndpointEnv)"));
    // The match lowers to a switch with the wildcard as default.
    assert!(serde.contains("switch s.EndpointVersion {"));
    assert!(serde.contains("case \"v1\":"));
    assert!(serde.contains("case \"legacy\":"));
    assert!(serde.contains("s.Endpoint = \"https://old.example.com\""));
    // An arm that reads an absent chain reports it at the point of use.
    assert!(serde.contains(
        "endpointErr = &ConfigError{Message: \"endpoint_v1 <- \" + endpointV1Err.Error(), Cause: endpointV1Err}"
    ));
    // @bind: the entry value feeds the composed member; the unbound member
    // keeps its own chain.
    assert!(serde.contains("composed.APIKey = s.APIKey"));
    assert!(serde.contains("composed.Region = \"us\""));
    // Nothing freezes into a runtime bag anymore: a wire position reads the
    // typed Settings at its own call site.
    assert!(!serde.contains("values :="));
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

/// The full typed wire binding the fixture ops carry under test: a labeled
/// path, a mixed body, a declared header, and the retry/timeout policy, so
/// one emission exercises every request position. The typechecker rejects an
/// entry @http op without an endpoint, so the fixture always carries one.
fn typed_wire() -> crate::ir::WireBinding {
    crate::ir::WireBinding {
        method: "POST".into(),
        uri: WireValue::Template(vec![
            TemplatePart::Lit("/notes/".into()),
            TemplatePart::Input("id".into()),
        ]),
        body: Some(WireValue::Object(vec![(
            "body".to_string(),
            WireValue::Param(vec!["body".into()]),
        )])),
        response_bindings: Default::default(),
        success: vec![200],
        endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
        request_headers: vec![(
            vec![TemplatePart::Lit("X-API-Key".into())],
            crate::ir::WireValue::Field(vec!["api_key".into()]),
        )],
        query: vec![],
        timeout: Some(vec!["timeout".into()]),
        retry: Some(vec!["max_retries".into()]),
    }
}

/// Attach `wire` to every entry op, standing in for the frontend's protocol
/// pass (the schema fixture is pre-protocol).
fn with_wire(mut module: Module, wire: crate::ir::WireBinding) -> Module {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                if let ShapeKind::Operation { wire: slot, .. } = &mut op.kind {
                    *slot = Some(Box::new(wire.clone()));
                }
            }
        }
    }
    module
}

fn with_descriptors(module: Module) -> Module {
    with_wire(module, typed_wire())
}

#[test]
fn the_method_assembles_the_request_and_maps_the_outcome_onto_the_taxonomy() {
    let module = with_descriptors(fixture_module());
    let serde = entry_text(&module);
    // No embedded descriptor blob: the request assembles from the typed wire
    // binding in the method's own text.
    assert!(!serde.contains("MustDescriptor"));
    assert!(serde.contains("record, err := record.EncodeRecord(input)"));
    assert!(serde.contains(
        "requestURL := c.settings.Endpoint + \"/notes/\" + transport.PathPart(record[\"id\"])"
    ));
    // Headers layer declared, then base, and the declared value reads the
    // typed settings directly.
    assert!(serde.contains("transport.SetHeader(headers, \"X-API-Key\", c.settings.APIKey)"));
    assert!(serde.contains("for name, value := range c.settings.Headers {"));
    // The @body ctor mapper builds an object of just its declared fields.
    assert!(serde.contains("body, err := json.Marshal(map[string]any{\"body\": record[\"body\"]})"));
    assert!(serde.contains(
        "outcome := transport.Send(ctx, c.settings.HTTPClient, c.settings.Transport, transport.Request{"
    ));
    // No hook is bound, so no dead hook-error check is emitted.
    assert!(!serde.contains("HookErr"));
    // @timeout reads the pre-converted client field the constructor built.
    assert!(serde.contains("Timeout: c.timeoutDuration,"));
    assert!(serde.contains("d, err := time.ParseDuration(string(s.Timeout))"));
    assert!(serde.contains("timeoutDuration = d"));
    // The retry policy and the decoded error type read the same
    // discriminator, so they can never disagree.
    assert!(serde.contains(
        "Retry: transport.Retry{Max: int(c.settings.MaxRetries), When: func(status int, body string) bool {"
    ));
    assert!(serde.contains(
        "if re, ok := DecodeSaveNoteError(status, []byte(body)).(interface{ Retryable() bool }); ok {"
    ));
    // The timing seam rides the client for the package's own tests to pin.
    assert!(serde.contains("Timing: c.timing,"));
    assert!(serde.contains("timing transport.Timing"));
    assert!(serde.contains("&TransportError{Cause: outcome.Cause}"));
    assert!(serde.contains("DecodeSaveNoteError(outcome.Status, []byte(outcome.Body))"));
    // The required-member probe lives once per type (DecodeNote); the call
    // site only routes the returned path into its own DecodeError.
    assert!(serde.contains("out, path, ok := DecodeNote([]byte(outcome.Body))"));
    assert!(serde.contains("&DecodeError{Path: path, Expected: \"Note\", Raw: outcome.Body}"));
}

#[test]
fn an_operation_with_no_retry_or_timeout_declares_neither() {
    let mut wire = typed_wire();
    wire.uri = WireValue::Template(vec![TemplatePart::Lit("/notes".into())]);
    wire.body = Some(WireValue::Param(vec![]));
    wire.request_headers = Vec::new();
    wire.retry = None;
    wire.timeout = None;
    let module = with_wire(fixture_module(), wire);
    let serde = entry_text(&module);
    // The all-body input marshals directly; no record indirection.
    assert!(serde.contains("body, err := json.Marshal(input)"));
    assert!(!serde.contains("EncodeRecord"));
    // No trace of the undeclared policies, in the method or on the client.
    assert!(!serde.contains("transport.Retry"));
    assert!(!serde.contains("Timing"));
    assert!(!serde.contains("Timeout:"));
    assert!(!serde.contains("timeoutDuration"));
    assert!(!serde.contains("time.ParseDuration"));
    assert!(!serde.contains("Hooks"));
    assert!(serde.contains(
        "outcome := transport.Send(ctx, c.settings.HTTPClient, c.settings.Transport, transport.Request{"
    ));
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
    assert!(serde.contains(
        "readerErr = &ConfigError{Message: \"naming <- \" + namingErr.Error(), Cause: namingErr}"
    ));
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
    assert!(serde.contains("if credsErr != nil {"));
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
        .rfind("if credsErr != nil {")
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
    // A wire position naming the field by its canonical path still reads the
    // renamed Go identifier.
    let mut wire = typed_wire();
    wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Auth".into())],
        crate::ir::WireValue::Field(vec!["primary_key".into()]),
    )];
    let wired = entry_text(&with_wire(module, wire));
    assert!(wired.contains("transport.SetHeader(headers, \"X-Auth\", c.settings.AuthToken)"));
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
fn an_error_var_nothing_reads_is_discarded_rather_than_left_unused() {
    // A chain no other field, trait, or bind consumes still records its
    // failure in an error var, but nothing reads it back, and Go rejects a
    // variable that is assigned and never read.
    let mut module = fixture_module();
    let env = crate::ir::Source::Env(crate::ir::EnvName::Name("SPARE_TOKEN".into()));
    let field = bare_entry_field("spare_token", Tref::Prim(Prim::String), vec![env]);
    push_entry_field(&mut module, field);
    let serde = entry_text(&module);
    assert!(serde.contains("var spareTokenErr error"));
    assert!(serde.contains("\t_ = spareTokenErr\n"));
    // A consumed chain is read by its own check, so it is left alone.
    assert!(!serde.contains("_ = endpointVersionErr"));
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
    assert!(serde.contains("s.Team = casing.StrSnake(s.Team)"));
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
    assert!(serde.contains("var settingsMaxConnsErr error"));
    // The require reads the reason, never the (possibly legitimately zero) value.
    assert!(serde.contains("if settingsMaxConnsErr != nil {"));
    assert!(serde.contains(
        "&ConfigError{Message: \"settings.max_conns <- \" + settingsMaxConnsErr.Error(), Cause: settingsMaxConnsErr}"
    ));
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
    let send = serde
        .find("outcome := transport.Send(ctx")
        .expect("send call");
    assert!(val < send);
}

#[test]
fn a_structured_output_decodes_strictly_on_required_members() {
    let module = with_descriptors(fixture_module());
    let serde = entry_text(&module);
    // The probe lives once per type, in DecodeNote: a required member (a
    // zero value is not a present value) missing or null fails, naming its
    // own path; a whole-body parse failure points at the root. Unknown
    // fields are still tolerated (a plain Unmarshal into the struct).
    assert!(serde.contains("var noteRequiredFields = []string{\"id\", \"body\"}"));
    assert!(serde.contains("func DecodeNote(raw []byte) (Note, string, bool) {"));
    assert!(serde.contains("var probe map[string]json.RawMessage"));
    assert!(serde.contains("for _, field := range noteRequiredFields {"));
    assert!(serde.contains("if rv, ok := probe[field]; !ok || string(rv) == \"null\" {"));
    assert!(serde.contains("return Note{}, \"$.\" + field, false"));
    assert!(serde.contains("return Note{}, \"$\", false"));
    assert!(serde.contains("var out Note"));
    // The call site routes DecodeNote's returned path into its own error,
    // never reemitting the probe.
    assert!(serde.contains("out, path, ok := DecodeNote([]byte(outcome.Body))"));
    assert!(serde.contains("&DecodeError{Path: path, Expected: \"Note\", Raw: outcome.Body}"));
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
    // An enum field is a branded string: cast at the boundary.
    assert!(serde.contains("s.Mode = Mode(v)"));
    // Guaranteed and error-tracked dynamic env names both spell one balanced run.
    assert!(serde.contains("os.LookupEnv(s.SureName)"));
    assert!(serde.contains(
        "dynamicErr = &ConfigError{Message: \"naming <- \" + namingErr.Error(), Cause: namingErr}"
    ));
    // Transforms compose innermost-first; the input placeholder renders empty.
    assert!(serde.contains("casing.StrUpperSnake(casing.StrPascal(casing.StrKebab(casing.StrSnake(strings.ToUpper(strings.ToLower(strings.TrimSpace("));
    // Both select flavors: error-tracked with an inline source arm (no reset
    // needed entering the case — the error var is still nil from the switch's
    // own opening), and a guaranteed one that fails construction on an
    // undeclared value.
    assert!(serde.contains("case 1:"));
    assert!(serde.contains("pickedErr = &ConfigError{Message: \"not configured\"}"));
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
    assert!(serde.contains("if endpointErr == nil {"));
    assert!(serde.contains("if endpointV1Err == nil {"));
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
    assert!(serde.contains("(portErr == nil || s.Port != 0) &&"));
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
fn an_enum_member_of_a_config_flattens_at_a_wire_position() {
    let mut module = fixture_module();
    with_enum_config_member(&mut module);
    let mut wire = typed_wire();
    wire.request_headers = vec![(
        vec![TemplatePart::Lit("X-Mode".into())],
        crate::ir::WireValue::Field(vec!["settings".into(), "mode".into()]),
    )];
    let serde = entry_text(&with_wire(module, wire));
    // A branded string flattens through string(...) rather than being
    // JSON-marshalled (which would quote it).
    assert!(serde
        .contains("transport.SetHeader(headers, \"X-Mode\", string(c.settings.Settings.Mode))"));
}

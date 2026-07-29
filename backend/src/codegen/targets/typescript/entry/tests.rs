use super::*;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::{
    bare_entry_field, push_config_member, push_entry_field, push_entry_op_trait, rendered,
    set_entry_op_outputs, with_bytes_and_constrained_port, with_derived_config_members,
    with_enum_config_member, with_member_select_on_absent_subject, with_structured_sources,
    with_transformed_chain_field,
};
use crate::ir::decode_model;

fn fixture_module() -> Module {
    let text = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ir-schema/fixtures/entries_client.json"
    ));
    let model = decode_model(text).expect("fixture decodes");
    model.modules.into_iter().next().expect("one module")
}

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

fn text(module: &Module) -> String {
    {
        let emission = emit(module, &ts_casing());
        let mut decls = emission.shared;
        decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
        rendered(&decls, &TsRules)
    }
}

#[test]
fn the_entry_class_replaces_the_generic_client_surface() {
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    // The class takes @arg positionally and @with as an optional config
    // object; the Settings expose the resolved fields and transport slots.
    assert!(out.contains("export class Client {"));
    // The resolved settings are private (as Go's client field is unexported):
    // immutable to the consumer once construction and the hook have run.
    assert!(out.contains("private readonly settings: Settings;"));
    assert!(!out.contains("  readonly settings: Settings;"));
    assert!(out.contains("constructor(apiKey: string, config: ClientConfig = {}) {"));
    assert!(out.contains("export interface Settings {"));
    assert!(out.contains("  fetch?: typeof fetch;"));
    assert!(out.contains("  transport?: CanonicalTransport;"));
    assert!(out.contains("  headers: Record<string, string>;"));
    assert!(out.contains("export interface ClientConfig {"));
    assert!(out.contains("  clientName?: string;"));
    // Construction-only config interface.
    // The config interface is construction-only, hidden (not exported).
    assert!(out.contains("interface Conf {"));
    assert!(!out.contains("export interface Conf {"));
    // The descriptor is embedded verbatim and the method maps the outcome.
    assert!(out.contains("const saveNoteDescriptor: WireDescriptor = JSON.parse("));
    assert!(out.contains("async saveNote(input: Note): Promise<Note> {"));
    assert!(out.contains("throw new TransportError(outcome.cause);"));
    assert!(out.contains("decodeSaveNoteError(outcome.status, outcome.body)"));
}

#[test]
fn the_resolution_mirrors_the_go_spelling() {
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    assert!(out.contains("s.apiKey = apiKey;"));
    assert!(out.contains("s.clientName = \"demo\";"));
    assert!(out.contains("s.clientKey = strUpperSnake((s.clientName).trim());"));
    assert!(out.contains("switch (s.endpointVersion) {"));
    assert!(out.contains("case \"v1\": {"));
    assert!(out.contains(
        "endpointErr = new ConfigError(`endpoint_v1 <- ${endpointV1Err.message}`, endpointV1Err);"
    ));
    assert!(out.contains("composed.apiKey = s.apiKey;"));
    // Values freeze under canonical dotted names; bigints narrow, the
    // duration flows in milliseconds.
    // A member read goes through its zero so an undefined draft member
    // freezes (or guards) exactly like Go's zero struct member.
    assert!(out.contains("values[\"settings.api_key\"] = (s.settings.apiKey ?? \"\");"));
    assert!(out.contains("values[\"timeout\"] = durationToMs(String(s.timeout));"));
    // Entry construction leaves baseUrl empty: the endpoint resolves per
    // operation from the descriptor's ref.
    assert!(out.contains(
            "this.options = { baseUrl: \"\", fetch: s.fetch, transport: s.transport, headers: s.headers, values };"
        ));
}

#[test]
fn the_settings_bridge_wires_client_init_by_mutation() {
    let mut module = with_descriptors(fixture_module());
    module.extensions = vec![crate::ir::Extension {
        name: "client_init".into(),
        kind: crate::ir::ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("ts".to_string(), "ext/ts/init.ts#initSettings".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }];
    let out = text(&module);
    assert!(out.contains("function wrapClientInit(settings: Settings): void {"));
    assert!(out.contains("initSettings(settings);"));
    assert!(out.contains("wrapClientInit(s);"));
    assert!(out.contains("throw new ContractError(\"client_init\", e);"));
    // Bridge before validation: init runs before the consumed-chain check.
    let init = out.find("wrapClientInit(s);").unwrap();
    let require = out.find("throw new ConfigError(\"endpoint <- \"").unwrap();
    assert!(init < require);
}

#[test]
fn structured_sources_decode_strictly_with_context() {
    let mut module = with_descriptors(fixture_module());
    // Attach a structured field referencing a wire struct with a required
    // member and a check.
    with_structured_sources(
        &mut module,
        vec![Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into()))],
    );
    let out = text(&module);
    assert!(out.contains("missing field token"));
    // An explicit null in a required member is as absent as a missing key
    // (the Go probe treats it the same way).
    assert!(out.contains("if (!(\"token\" in parsed) || record[\"token\"] === null) {"));
    assert!(out.contains("unknown field ${key}"));
    assert!(out.contains("const decoded = decodeCredentials(parsed);"));
    assert!(out.contains("const invalid = validateCredentials(decoded);"));
    assert!(out.contains("throw invalid;"));
    // Required members are checked before unknown fields (Go's order), and
    // scalar wire types are checked so a mistyped member fails the same
    // way the typed Go decode does.
    let required = out.find("missing field token").unwrap();
    let unknown = out.find("unknown field ${key}").unwrap();
    assert!(required < unknown);
    assert!(out.contains("field token must be a string"));
}

#[test]
fn a_structured_source_falls_back_across_multiple_envs() {
    let mut module = with_descriptors(fixture_module());
    // Two @env sources: the second is a fallback tried only while the first is
    // still absent (a first-present-wins cascade, not first-only).
    with_structured_sources(
        &mut module,
        vec![
            Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into())),
            Source::Env(EnvName::Name("SERVICE_CREDENTIALS_FALLBACK".into())),
        ],
    );
    let out = text(&module);
    // Both variables are read (the fallback is not dropped).
    assert!(out.contains("readEnv(\"SERVICE_CREDENTIALS\")"));
    assert!(out.contains("readEnv(\"SERVICE_CREDENTIALS_FALLBACK\")"));
    // The fallback decode runs only while the first source stayed unresolved.
    let fallback = out
        .find("readEnv(\"SERVICE_CREDENTIALS_FALLBACK\")")
        .expect("fallback lookup");
    let guard = out[..fallback]
        .rfind("if (credsErr !== undefined) {")
        .expect("fallback guard");
    let primary = out
        .find("readEnv(\"SERVICE_CREDENTIALS\")")
        .expect("primary lookup");
    assert!(primary < guard && guard < fallback);
}

#[test]
fn a_consumed_numeric_config_member_requires_its_resolution_not_its_zero() {
    let mut module = with_descriptors(fixture_module());
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
    let out = text(&module);
    // The reason var is hoisted above the config block so the require can read it.
    assert!(out.contains("let settingsMaxConnsErr: ConfigError | undefined;"));
    // The require reads the reason, never the (possibly legitimately zero) value.
    assert!(out.contains("if (settingsMaxConnsErr !== undefined) {"));
    assert!(out.contains(
        "throw new ConfigError(\"settings.max_conns <- \" + settingsMaxConnsErr.message, settingsMaxConnsErr);"
    ));
    // It is not compared against the numeric zero (that would reject a real 0).
    assert!(!out.contains("s.settings.maxConns === 0"));
}

#[test]
fn a_structured_decode_probes_the_wire_key_not_the_member_name() {
    let mut module = with_descriptors(fixture_module());
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
                // @wire renames the serialized key; the decode must check "tok".
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
    let out = text(&module);
    // The required and unknown checks read the wire key, not the member name.
    assert!(out.contains("if (!(\"tok\" in parsed) || record[\"tok\"] === null) {"));
    assert!(out.contains("missing field tok"));
    assert!(!out.contains("\"token\" in parsed"));
}

#[test]
fn a_constrained_op_input_is_validated_before_transport() {
    let mut module = with_descriptors(fixture_module());
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
    let out = text(&module);
    // The input is validated and a violation surfaces as the Validation category
    // the validator itself returns.
    assert!(out.contains("const invalid = validateNote(input);"));
    assert!(out.contains("throw invalid;"));
    // The check runs before the transport call, not after.
    let val = out.find("validateNote(input)").expect("validate call");
    let exec = out.find("await execute(").expect("execute call");
    assert!(val < exec);
}

#[test]
fn a_structured_output_decodes_strictly_on_required_members() {
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    // The probe lives once per type, in parseNote: a required member (a
    // missing key or a null value counts as absent) fails, naming its own
    // path; a whole-body parse failure points at the root. Unknown fields
    // are still tolerated (decodeNote maps only what it knows).
    assert!(out.contains("const noteRequiredFields = [\"id\", \"body\"] as const;"));
    assert!(out.contains("function parseNote(raw: string): Note {"));
    assert!(out.contains("for (const field of noteRequiredFields) {"));
    assert!(out.contains("if (!(field in obj) || obj[field] === null) {"));
    assert!(out.contains("throw \"$.\" + field;"));
    assert!(out.contains("throw \"$\";"));
    assert!(out.contains("return decodeNote(obj);"));
    // The call site routes parseNote's thrown path into its own error, never
    // reemitting the probe.
    assert!(out.contains("return parseNote(outcome.body);"));
    assert!(out.contains("} catch (path) {"));
    assert!(out.contains("throw new DecodeError(path as string, \"Note\", outcome.body);"));
}

#[test]
fn the_constructor_rejects_conflicting_transports_at_construction() {
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    // The exclusive-transport check runs once at construction (as Go does in
    // New), not lazily on each call, so a misconfigured client fails to build.
    assert!(out.contains("assertExclusiveTransport(this.options);"));
    // The check sits in the constructor, before any operation method.
    let assert_at = out
        .find("assertExclusiveTransport(this.options)")
        .expect("assert call");
    let first_op = out.find("async saveNote(").expect("op method");
    assert!(assert_at < first_op);
}

#[test]
fn field_docs_flow_onto_the_settings_and_config_properties() {
    // client_name carries @doc in the fixture; it must land as JSDoc on both its
    // public surfaces (the Settings interface and the config object).
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    assert!(out.contains("  /** Names the caller. */\n  clientName: string;"));
    assert!(out.contains("  /** Names the caller. */\n  clientName?: string;"));
}

#[test]
fn an_entry_field_rename_retargets_every_ts_identifier() {
    let mut module = with_descriptors(fixture_module());
    let mut token = bare_entry_field("primary_key", Tref::Prim(Prim::String), vec![Source::Arg]);
    // @rename(lang) is a verbatim identifier, used at every position.
    token.traits = vec![crate::ir::Trait {
        id: "rename".into(),
        value: serde_json::json!({"typescript": "authToken"}),
    }];
    push_entry_field(&mut module, token);
    let out = text(&module);
    // The Settings property, the constructor param, and the internal write all
    // use the renamed identifier; none use the casing of the canonical name.
    assert!(out.contains("  authToken: string;"));
    assert!(out.contains("authToken: string"));
    assert!(out.contains("s.authToken = authToken;"));
    assert!(!out.contains("primaryKey"));
    // The canonical dotted key the runtime reads is unchanged.
    assert!(out.contains("values[\"primary_key\"]"));
}

/// An `ext impl` binding for the fixture's `save_note`, typed or raw.
fn impl_ext(raw: bool) -> crate::ir::Extension {
    crate::ir::Extension {
        name: "save_note".into(),
        kind: crate::ir::ExtKind::Impl,
        signature: None,
        raw,
        bindings: [("ts".to_string(), "ext/ts/save.ts#saveNote".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }
}

#[test]
fn an_operation_with_neither_a_descriptor_nor_an_impl_fails_loudly() {
    // The fixture is pre-protocol (no descriptor) and binds no impl, a
    // combination the emit gate refuses; a direct library caller that skipped
    // it gets a diagnosable method that still takes the declared input.
    let module = fixture_module();
    let out = text(&module);
    assert!(out.contains("async saveNote(input: Note): Promise<Note> {"));
    assert!(out.contains("operation has no implementation for TypeScript"));
}

#[test]
fn a_typed_impl_calls_the_bound_symbol_and_guards_the_error_boundary() {
    let mut module = fixture_module();
    module.extensions = vec![impl_ext(false)];
    let out = text(&module);
    assert!(out.contains("return await saveNote(this.settings, input);"));
    // A declared SDK error crosses typed; anything else is named.
    assert!(out.contains("} catch (e) {"));
    assert!(out.contains("if (e instanceof TonoError) throw e;"));
    assert!(out.contains("throw new ContractError(\"save_note\", e);"));
    // The typed form needs no discrimination: declared errors arrive typed.
    assert!(!out.contains("function decodeSaveNoteError("));
    // The expected bespoke signature is documented above the method.
    assert!(out.contains("function saveNote(settings: Settings, input: Note): Promise<Note>"));
}

#[test]
fn a_raw_impl_decodes_the_outcome_and_discriminates_by_code() {
    let mut module = fixture_module();
    module.extensions = vec![impl_ext(true)];
    let out = text(&module);
    // The input travels as its wire text; the outcome comes back raw.
    assert!(
        out.contains("outcome = await saveNote(this.settings, JSON.stringify(encodeNote(input)));")
    );
    // A failing outcome discriminates on the code alone: a bespoke
    // implementation carries no protocol status.
    assert!(out.contains("if (!outcome.success) {"));
    assert!(out.contains("throw decodeSaveNoteError(outcome.code, outcome.body);"));
    assert!(out.contains("code === CODE_OVERLOADED"));
    assert!(out.contains("return new APIError(0, body);"));
    // The success payload decodes exactly as a protocol response does,
    // through the same per-type parseNote a protocol operation returning
    // Note shares.
    assert!(out.contains("return parseNote(outcome.body);"));
    assert!(out.contains("new DecodeError(path as string, \"Note\", outcome.body)"));
    assert!(
        out.contains("function saveNote(settings: Settings, payload: string): Promise<Outcome>")
    );
}

#[test]
fn a_guaranteed_chain_reads_each_env_variable_once() {
    let mut module = with_descriptors(fixture_module());
    push_entry_field(
        &mut module,
        bare_entry_field(
            "my_region",
            Tref::Prim(Prim::String),
            vec![
                Source::Env(EnvName::Name("MY_REGION".into())),
                Source::Default(serde_json::json!("us")),
            ],
        ),
    );
    let out = text(&module);
    assert_eq!(out.matches("readEnv(\"MY_REGION\")").count(), 1);
    assert!(out.contains("let myRegionSet = false;"));
    assert!(out.contains("if (!myRegionSet) {"));
}

#[test]
fn a_module_without_entries_emits_nothing() {
    let module = Module {
        name: "m".into(),
        shapes: vec![],
        operations: vec![],
        extensions: vec![],
    };
    let emission = emit(&module, &ts_casing());
    assert!(emission.shared.is_empty() && emission.per_entry.is_empty());
}

#[test]
fn the_matrix_module_exercises_every_resolution_idiom() {
    let module = crate::codegen::test_support::entries_matrix_module();
    let out = text(&module);
    // Typed boundaries: digit grammar, sign, and per-type ranges.
    assert!(out.contains("n < -128 || n > 127"));
    assert!(out.contains("n < -32768 || n > 32767"));
    assert!(out.contains("n > 255"));
    assert!(out.contains("-9223372036854775808n"));
    assert!(out.contains("18446744073709551615n"));
    assert!(out.contains("Number.isFinite(n)"));
    assert!(out.contains("durationToMs(v)"));
    assert!(out.contains("v === \"true\" || v === \"1\""));
    // An enum field is a branded string: cast at the boundary, frozen.
    assert!(out.contains("s.mode = v as Mode;"));
    assert!(out.contains("values[\"mode\"] = s.mode;"));
    // Guaranteed and error-tracked dynamic env names.
    assert!(out.contains("readEnv(s.sureName)"));
    assert!(
        out.contains("dynamicErr = new ConfigError(`naming <- ${namingErr.message}`, namingErr);")
    );
    // Transforms compose innermost-first; the input placeholder renders empty.
    assert!(out.contains("strUpperSnake(strPascal(strKebab(strSnake(("));
    // Both select flavors, the guaranteed one failing on an undeclared value.
    assert!(out.contains("case 1: {"));
    assert!(out.contains(
        "throw new ConfigError(`sure_pick: match on sure_name: unmatched value ${String(s.sureName)}`);"
    ));
    // Composition with member chains, including an int member parse.
    assert!(out.contains("composed.key = s.naming;"));
    assert!(out.contains("composed.sure = s.sureName;"));
    // Structured and whole-JSON sources honor explicit values.
    assert!(out.contains("if (config.creds !== undefined) {"));
    assert!(out.contains("s.credsArg = credsArg;"));
    assert!(out.contains("s.tags = tags;"));
    // The four method shapes: full descriptor, bare, primitive output, stub.
    assert!(out.contains("async fetchNote(input: Note): Promise<Note> {"));
    assert!(out.contains("async ping(): Promise<void> {"));
    assert!(out.contains("async count(): Promise<number> {"));
    assert!(out.contains("async local(input: Note): Promise<string> {"));
    assert!(out.contains("operation has no implementation for TypeScript"));
}

#[test]
fn transforms_apply_to_chain_and_match_resolved_values() {
    let mut module = fixture_module();
    with_transformed_chain_field(&mut module);
    let out = text(&module);
    // The pipeline runs over the resolved value whatever idiom produced it.
    assert!(out.contains("s.team = strSnake(s.team);"));
}

#[test]
fn a_config_member_keeps_its_declared_derivation() {
    let mut module = fixture_module();
    with_derived_config_members(&mut module);
    let out = text(&module);
    // The member's @format (with its transforms) lands inside the composition.
    assert!(out.contains("composed.label = (\"conf-\" + s.clientName).toUpperCase();"));
    // The member's match lowers to a switch writing the composed member; an
    // unmatched value leaves the zero (no member-level why to track).
    assert!(out.contains("case \"demo\": {"));
    assert!(out.contains("composed.size = \"small\";"));
}

#[test]
fn the_env_boundary_decodes_bytes_and_rejects_non_decimal_numbers() {
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
    push_entry_field(
        &mut module,
        bare_entry_field(
            "attempts",
            Tref::Prim(Prim::U64),
            vec![Source::Env(EnvName::Name("ATTEMPTS".into()))],
        ),
    );
    let out = text(&module);
    // Bytes ride the env boundary as base64, through the wire codec helper;
    // the draft starts from an empty buffer, not a lying cast.
    assert!(out.contains("s.secret = decodeBytes(v);"));
    assert!(out.contains("invalid base64"));
    assert!(out.contains("secret: new Uint8Array(), "));
    // Floats take decimal notation only (no Inf/hex), like the Go boundary.
    assert!(out.contains("if (!/^[+-]?(\\d+(\\.\\d*)?|\\.\\d+)([eE][+-]?\\d+)?$/.test(v)) {"));
    // Unsigned integers take no sign at all (ParseUint's rule).
    assert!(out.contains("if (!/^[0-9]+$/.test(v)) {"));
}

#[test]
fn a_whole_json_field_decodes_through_the_wire_codecs() {
    let mut module = with_descriptors(fixture_module());
    push_entry_field(
        &mut module,
        bare_entry_field(
            "quotas",
            Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::I64)),
            ),
            vec![Source::Env(EnvName::Name("QUOTAS".into()))],
        ),
    );
    let out = text(&module);
    // The parsed JSON runs through the same decode the wire codec uses, so
    // an i64 map lands as bigints instead of raw strings.
    assert!(out.contains("parsed = JSON.parse(raw);"));
    assert!(out.contains("[k, decodeI64(v)]"));
    // The boundary is as strict as Go's typed unmarshal: the container shape
    // and every scalar element are checked before the decode (i64 rides the
    // wire as a string).
    assert!(out
        .contains("if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed))"));
    assert!(out.contains("for (const [key, val] of Object.entries(parsed)) {"));
    assert!(out.contains("field ${key} must be a string"));
}

#[test]
fn a_consumed_config_member_requires_a_value_at_construction() {
    let mut module = fixture_module();
    push_entry_op_trait(
        &mut module,
        "header",
        serde_json::json!(["X-Key", {"field": ["settings", "api_key"]}]),
    );
    let out = text(&module);
    // The leaf value itself is checked (there is no member-level why), read
    // through its zero so an undefined draft member counts as absent.
    assert!(out.contains("if ((s.settings.apiKey ?? \"\") === \"\") {"));
    assert!(out.contains("throw new ConfigError(\"settings.api_key: no value\");"));
}

#[test]
fn duration_parsing_accepts_both_micro_signs() {
    // The helper is the SDK's, not the entry's, so it is read where it lives.
    let out = rendered(&resolution_helpers(), &TsRules);
    // Go's ParseDuration takes U+00B5 and U+03BC; the shared grammar must too.
    assert!(out.contains("\\u00b5s"));
    assert!(out.contains("\\u03bcs"));
}

#[test]
fn a_config_member_match_tracks_absent_subjects_and_inline_sources() {
    let mut module = fixture_module();
    with_member_select_on_absent_subject(&mut module);
    let out = text(&module);
    // The member's switch only runs once the why-tracked subject resolved, an
    // arm reading an absent chain assigns only when that chain resolved, and
    // an inline source arm keeps the presence-only member spelling.
    assert!(out.contains("if (endpointErr === undefined) {"));
    assert!(out.contains("if (endpointV1Err === undefined) {"));
    assert!(out.contains("readEnv(\"ZONE\")"));
}

#[test]
fn a_consumed_bytes_head_requires_a_value_and_numeric_constraints_gate_on_presence() {
    let mut module = fixture_module();
    with_bytes_and_constrained_port(&mut module);
    let out = text(&module);
    assert!(out.contains("if (s.secret.length === 0) {"));
    // The numeric constraint skips when the chain reported absent and the
    // bridge left the zero in place (same presence rule as the requires).
    assert!(out.contains("(portErr === undefined || s.port !== 0) &&"));
}

#[test]
fn a_64_bit_operation_output_decodes_from_its_wire_string() {
    let mut module = with_descriptors(fixture_module());
    set_entry_op_outputs(&mut module, Tref::Prim(Prim::I64));
    let out = text(&module);
    assert!(out.contains("return decodeI64(JSON.parse(outcome.body)) as bigint;"));
}

#[test]
fn an_enum_member_of_a_config_freezes_as_a_branded_string() {
    let mut module = fixture_module();
    with_enum_config_member(&mut module);
    let out = text(&module);
    // The member reads through the branded empty string, never the draft's
    // empty-object spelling.
    assert!(out.contains("values[\"settings.mode\"] = (s.settings.mode ?? \"\" as Mode);"));
}

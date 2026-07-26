use super::*;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::rendered;
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
    rendered(&entry_decls(module, &ts_casing()), &TsRules)
}

#[test]
fn the_entry_class_replaces_the_generic_client_surface() {
    let module = with_descriptors(fixture_module());
    let out = text(&module);
    // The class takes @arg positionally and @with as an optional config
    // object; the Settings expose the resolved fields and transport slots.
    assert!(out.contains("export class Client {"));
    assert!(out.contains("constructor(apiKey: string, config: ClientConfig = {}) {"));
    assert!(out.contains("export interface Settings {"));
    assert!(out.contains("  fetch?: typeof fetch;"));
    assert!(out.contains("  transport?: CanonicalTransport;"));
    assert!(out.contains("  headers: Record<string, string>;"));
    assert!(out.contains("export interface ClientConfig {"));
    assert!(out.contains("  clientName?: string;"));
    // Construction-only config interface.
    assert!(out.contains("export interface Conf {"));
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
    assert!(out.contains("endpointWhy = \"endpoint_v1 <- \" + endpointV1Why;"));
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
    let require = out.find("throw new Error(\"endpoint <- \"").unwrap();
    assert!(init < require);
}

#[test]
fn structured_sources_decode_strictly_with_context() {
    let mut module = with_descriptors(fixture_module());
    // Attach a structured field referencing a wire struct with a required
    // member and a check.
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
            fields.push(EntryField {
                name: "creds".into(),
                target: Tref::Ref {
                    id: "notes#credentials".into(),
                    args: vec![],
                },
                sources: vec![Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let out = text(&module);
    assert!(out.contains("missing field token"));
    // An explicit null in a required member is as absent as a missing key
    // (the Go probe treats it the same way).
    assert!(out.contains("if (!(\"token\" in parsed) || record[\"token\"] === null) {"));
    assert!(out.contains("unknown field ${key}"));
    assert!(out.contains("const decoded = decodeCredentials(parsed);"));
    assert!(out.contains("const vs = validateCredentials(decoded);"));
    assert!(out.contains("throw new ValidationError(vs);"));
    // Required members are checked before unknown fields (Go's order), and
    // scalar wire types are checked so a mistyped member fails the same
    // way the typed Go decode does.
    let required = out.find("missing field token").unwrap();
    let unknown = out.find("unknown field ${key}").unwrap();
    assert!(required < unknown);
    assert!(out.contains("field token must be a string"));
}

#[test]
fn a_bespoke_stub_keeps_the_declared_signature() {
    // No descriptor on the op (the fixture is pre-protocol): the stub
    // still takes the declared input.
    let module = fixture_module();
    let out = text(&module);
    assert!(out.contains("async saveNote(input: Note): Promise<Note> {"));
    assert!(out.contains("operation has no transport binding"));
}

#[test]
fn a_guaranteed_chain_reads_each_env_variable_once() {
    let mut module = with_descriptors(fixture_module());
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(EntryField {
                name: "my_region".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![
                    Source::Env(EnvName::Name("MY_REGION".into())),
                    Source::Default(serde_json::json!("us")),
                ],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
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
    assert!(entry_decls(&module, &ts_casing()).is_empty());
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
    // Guaranteed and why-tracked dynamic env names.
    assert!(out.contains("readEnv(s.sureName)"));
    assert!(out.contains("dynamicWhy = \"naming <- \" + namingWhy;"));
    // Transforms compose innermost-first; the input placeholder renders empty.
    assert!(out.contains("strUpperSnake(strPascal(strKebab(strSnake(("));
    // Both select flavors, the guaranteed one failing on an undeclared value.
    assert!(out.contains("case 1: {"));
    assert!(out.contains(
        "throw new Error(`sure_pick: match on sure_name: unmatched value ${String(s.sureName)}`);"
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
    assert!(out.contains("operation has no transport binding"));
}

#[test]
fn transforms_apply_to_chain_and_match_resolved_values() {
    let mut module = fixture_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(EntryField {
                name: "team".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![Source::Env(EnvName::Name("TEAM".into()))],
                format: None,
                transforms: vec!["snake".into()],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let out = text(&module);
    // The pipeline runs over the resolved value whatever idiom produced it.
    assert!(out.contains("s.team = strSnake(s.team);"));
}

#[test]
fn a_config_member_keeps_its_declared_derivation() {
    let mut module = fixture_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Config { fields } = &mut shape.kind {
            fields.push(EntryField {
                name: "label".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![],
                format: Some(vec![
                    crate::ir::TemplatePart::Lit("conf-".into()),
                    crate::ir::TemplatePart::Field(vec!["client_name".into()]),
                ]),
                transforms: vec!["upper".into()],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(EntryField {
                name: "size".into(),
                target: Tref::Prim(Prim::String),
                sources: vec![],
                format: None,
                transforms: vec![],
                select: Some(crate::ir::Select {
                    subject: vec!["client_name".into()],
                    arms: vec![crate::ir::SelectArm {
                        pattern: Some(serde_json::json!("demo")),
                        value: crate::ir::ArmValue::Lit(serde_json::json!("small")),
                    }],
                }),
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
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
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(EntryField {
                name: "secret".into(),
                target: Tref::Prim(Prim::Bytes),
                sources: vec![Source::Env(EnvName::Name("SECRET".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(EntryField {
                name: "rate".into(),
                target: Tref::Prim(Prim::Float),
                sources: vec![Source::Env(EnvName::Name("RATE".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(EntryField {
                name: "attempts".into(),
                target: Tref::Prim(Prim::U64),
                sources: vec![Source::Env(EnvName::Name("ATTEMPTS".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
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
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(EntryField {
                name: "quotas".into(),
                target: Tref::Map(
                    Box::new(Tref::Prim(Prim::String)),
                    Box::new(Tref::Prim(Prim::I64)),
                ),
                sources: vec![Source::Env(EnvName::Name("QUOTAS".into()))],
                format: None,
                transforms: vec![],
                select: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    let out = text(&module);
    // The parsed JSON runs through the same decode the wire codec uses, so
    // an i64 map lands as bigints instead of raw strings.
    assert!(out.contains("const parsed = JSON.parse(raw);"));
    assert!(out.contains("[k, decodeI64(v)]"));
}

#[test]
fn a_consumed_config_member_requires_a_value_at_construction() {
    let mut module = fixture_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                op.traits.push(crate::ir::Trait {
                    id: "header".into(),
                    value: serde_json::json!(["X-Key", {"field": ["settings", "api_key"]}]),
                });
            }
        }
    }
    let out = text(&module);
    // The leaf value itself is checked (there is no member-level why), read
    // through its zero so an undefined draft member counts as absent.
    assert!(out.contains("if ((s.settings.apiKey ?? \"\") === \"\") {"));
    assert!(out.contains("throw new Error(\"settings.api_key: no value\");"));
}

#[test]
fn duration_parsing_accepts_both_micro_signs() {
    let module = crate::codegen::test_support::entries_matrix_module();
    let out = text(&module);
    // Go's ParseDuration takes U+00B5 and U+03BC; the shared grammar must too.
    assert!(out.contains("\\u00b5s"));
    assert!(out.contains("\\u03bcs"));
}

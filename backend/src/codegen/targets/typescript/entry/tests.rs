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
    assert!(out.contains("values[\"settings.api_key\"] = s.settings.apiKey;"));
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

//! Codegen snapshot review gate. The whole emitted SDK for the shared
//! wire-matrix module (its groups, their paths, and their source) is snapshotted
//! per target. An unexpected diff fails the test; an intended change is accepted
//! with `cargo insta review`, so a human signs off on every codegen change,
//! layout included.
//!
//! Generation goes through the same path the CLI uses, with the engine's own
//! rough output (no language formatter need be installed), so the snapshot is
//! hermetic and deterministic.

use insta::assert_snapshot;
use tono_backend::codegen::{casing_for, generate_target, CodegenConfig, TargetKind};
use tono_backend::ir::{Model, Module, WireValue};

mod common;
use common::matrix_module;

/// Generate one target's whole SDK for a single-module model and lay every file
/// out in one snapshot body, each tagged with its output path.
fn render_sdk(module: Module, target: TargetKind, go_module: Option<&str>) -> String {
    let model = Model {
        tono_ir_version: 6,
        modules: vec![module],
    };
    let config = CodegenConfig {
        go_module: go_module.map(str::to_string),
        ..CodegenConfig::default()
    };
    let files = generate_target(&model, target, &config, &casing_for(target)).expect("generates");
    let mut out = String::new();
    for file in files {
        out.push_str(&format!(
            "// ==== {} ====\n{}\n",
            file.path
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/"),
            file.text
        ));
    }
    out
}

#[test]
fn rust_codegen_snapshot() {
    assert_snapshot!("rust", render_sdk(matrix_module(), TargetKind::Rust, None));
}

#[test]
fn go_codegen_snapshot() {
    assert_snapshot!("go", render_sdk(matrix_module(), TargetKind::Go, None));
}

#[test]
fn typescript_codegen_snapshot() {
    assert_snapshot!(
        "typescript",
        render_sdk(matrix_module(), TargetKind::TypeScript, None)
    );
}

/// The entry fixture (config, every source kind, derivation, selection,
/// composition, structured protocol refs) with an opaque descriptor attached
/// to its nested op, standing in for the frontend's protocol pass. The
/// snapshot covers the whole construction surface: New, With*, Settings, the
/// match switch, @bind composition, and the outcome mapping.
fn entries_module() -> tono_backend::ir::Module {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ir-schema/fixtures/entries_client.json"
    ))
    .expect("fixture");
    let model = tono_backend::ir::decode_model(&text).expect("fixture decodes");
    let mut module = model.modules.into_iter().next().expect("one module");
    // A structured source (JSON in an env variable) and a whole-JSON map, so
    // the golden pins the strict decode path (required probe, unknown-field
    // rejection, declared validation) in both targets.
    module.shapes.push(tono_backend::ir::Shape {
        id: "notes#credentials".into(),
        kind: tono_backend::ir::ShapeKind::Structure {
            params: vec![],
            members: vec![
                tono_backend::ir::Member {
                    name: "token".into(),
                    target: tono_backend::ir::Tref::Prim(tono_backend::ir::Prim::String),
                    required: true,
                    default: None,
                    constraints: vec![tono_backend::ir::Constraint::Length {
                        min: Some(1),
                        max: None,
                    }],
                    traits: vec![],
                },
                tono_backend::ir::Member {
                    name: "account_id".into(),
                    target: tono_backend::ir::Tref::Prim(tono_backend::ir::Prim::String),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                },
            ],
        },
        traits: vec![],
    });
    for shape in &mut module.shapes {
        if let tono_backend::ir::ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(tono_backend::ir::EntryField {
                name: "creds".into(),
                target: tono_backend::ir::Tref::Ref {
                    id: "notes#credentials".into(),
                    args: vec![],
                },
                sources: vec![tono_backend::ir::Source::Env(
                    tono_backend::ir::EnvName::Name("SERVICE_CREDENTIALS".into()),
                )],
                format: None,
                transforms: vec![],
                select: None,
                call: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
            fields.push(tono_backend::ir::EntryField {
                name: "labels".into(),
                target: tono_backend::ir::Tref::Map(
                    Box::new(tono_backend::ir::Tref::Prim(tono_backend::ir::Prim::String)),
                    Box::new(tono_backend::ir::Tref::Prim(tono_backend::ir::Prim::String)),
                ),
                sources: vec![tono_backend::ir::Source::Env(
                    tono_backend::ir::EnvName::Name("SERVICE_LABELS".into()),
                )],
                format: None,
                transforms: vec![],
                select: None,
                call: None,
                binds: vec![],
                constraints: vec![],
                traits: vec![],
            });
        }
    }
    for shape in &mut module.shapes {
        if let tono_backend::ir::ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                // The typed wire binding every target reads directly.
                if let tono_backend::ir::ShapeKind::Operation { wire, .. } = &mut op.kind {
                    *wire = Some(Box::new(tono_backend::ir::WireBinding {
                        method: "POST".into(),
                        uri: WireValue::Template(vec![
                            tono_backend::ir::TemplatePart::Lit("/notes/".into()),
                            tono_backend::ir::TemplatePart::Input("id".into()),
                        ]),
                        body: Some(WireValue::Param(vec!["body".into()])),
                        response_bindings: Default::default(),
                        success: vec![200],
                        endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
                        request_headers: vec![(
                            vec![tono_backend::ir::TemplatePart::Lit("X-Client-Name".into())],
                            tono_backend::ir::WireValue::Field(vec!["client_name".into()]),
                        )],
                        query: vec![],
                        timeout: Some(vec!["timeout".into()]),
                        retry: Some(vec!["max_retries".into()]),
                    }));
                }
            }
        }
    }
    module
}

#[test]
fn rust_entries_codegen_snapshot() {
    assert_snapshot!(
        "rust_entries",
        render_sdk(entries_module(), TargetKind::Rust, None)
    );
}

#[test]
fn go_entries_codegen_snapshot() {
    assert_snapshot!(
        "go_entries",
        render_sdk(entries_module(), TargetKind::Go, Some("example.com/sdk"))
    );
}

#[test]
fn typescript_entries_codegen_snapshot() {
    assert_snapshot!(
        "typescript_entries",
        render_sdk(entries_module(), TargetKind::TypeScript, None)
    );
}

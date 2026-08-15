//! The IR model the round-trip test (`tests/rust_ext_roundtrip.rs`) compiles
//! against a real stand-in crate under `codegen-tests/rust-ext/fixtures/`:
//! the RFC appendix's `companyconfig` library (a config load with a
//! per-field `yields`/`returns` projection and a `match`), scoped to the
//! extern-call field source this target emits. The appendix's injectable
//! `companybus` handle is not included: Rust does not emit a foreign
//! opaque-handle type yet (`TargetKind::emits_ext_handle_types`), so a
//! model exercising it would not validate for this target.

use crate::ir::Prim;
use crate::ir::{
    ArmValue, CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam,
    ForeignField, ForeignStruct, LangPath, Member, Model, Module, ReturnsField, ReturnsLit,
    ReturnsValue, Select, SelectArm, Shape, ShapeKind, Source, Tref, YieldsPos, TONO_IR_VERSION,
};

fn string_field(name: &str, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target: Tref::Prim(Prim::String),
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn member(name: &str) -> Member {
    Member {
        name: name.into(),
        target: Tref::Prim(Prim::String),
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

/// The RFC appendix's `companyconfig.load`, scoped to the extern-call
/// field source: an entry `config` field reads `service`/`region`, the
/// library returns a shape whose `Env` is matched to pick `Host`/`DevHost`,
/// and `token` reads `Credentials.Secret`.
pub fn rust_ext_fixture_model() -> Model {
    let service = string_field("service", vec![Source::Default(serde_json::json!("notes"))]);
    let region = string_field("region", vec![Source::Arg]);
    let mut config = string_field("config", vec![]);
    config.target = Tref::Ref {
        id: "m#app_config".into(),
        args: vec![],
    };
    config.call = Some(EntryCall {
        ns: "companyconfig".into(),
        func: "load".into(),
        args: vec![
            CallArg::Ref(vec!["service".into()]),
            CallArg::Ref(vec!["region".into()]),
        ],
    });

    let app_config = Shape {
        id: "m#app_config".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![member("endpoint"), member("token")],
        },
        traits: vec![],
    };

    let client = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![service, region, config],
            operations: vec![],
        },
        traits: vec![],
    };

    let ext_lib = ExtLib {
        name: "companyconfig".into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "companyconfig".into(),
        }],
        structs: vec![
            ForeignStruct {
                name: "Creds".into(),
                fields: vec![ForeignField {
                    name: "secret".into(),
                    r#type: Tref::Prim(Prim::String),
                }],
            },
            ForeignStruct {
                name: "Config".into(),
                fields: vec![
                    ForeignField {
                        name: "host".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                    ForeignField {
                        name: "dev_host".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                    ForeignField {
                        name: "env".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                    ForeignField {
                        name: "credentials".into(),
                        r#type: Tref::Ref {
                            id: "companyconfig#Creds".into(),
                            args: vec![],
                        },
                    },
                ],
            },
        ],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: vec![
                ExternParam {
                    name: "service".into(),
                    r#type: Tref::Prim(Prim::String),
                },
                ExternParam {
                    name: "region".into(),
                    r#type: Tref::Prim(Prim::String),
                },
            ],
            r#return: Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "rust".into(),
                symbol: "load".into(),
                call_args: vec![
                    CallArg::Ref(vec!["service".into()]),
                    CallArg::Ref(vec!["region".into()]),
                ],
                yields: vec![YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(Tref::Ref {
                        id: "companyconfig#Config".into(),
                        args: vec![],
                    }),
                    is_error: false,
                }],
                returns: Some(ReturnsLit {
                    r#type: Tref::Ref {
                        id: "m#app_config".into(),
                        args: vec![],
                    },
                    fields: vec![
                        ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Select(Select {
                                subject: vec!["cfg".into(), "env".into()],
                                arms: vec![
                                    SelectArm {
                                        pattern: Some(serde_json::json!("prod")),
                                        value: ArmValue::Field(vec!["cfg".into(), "host".into()]),
                                    },
                                    SelectArm {
                                        pattern: None,
                                        value: ArmValue::Field(vec![
                                            "cfg".into(),
                                            "dev_host".into(),
                                        ]),
                                    },
                                ],
                            }),
                        },
                        ReturnsField {
                            name: "token".into(),
                            value: ReturnsValue::Field(vec![
                                "cfg".into(),
                                "credentials".into(),
                                "secret".into(),
                            ]),
                        },
                    ],
                }),
                errors: vec![],
            }],
        }],
    };

    let module = Module {
        name: "m".into(),
        shapes: vec![client, app_config],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![ext_lib],
        tests: vec![],
    };
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::pipeline::generate_target;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::{CodegenConfig, TargetKind};

    /// `tests/rust_ext_roundtrip.rs` (the real verification: the fixture
    /// crate compiling the emitted call) skips under `cargo-llvm-cov`, since
    /// coverage instrumentation and a real `cargo build` subprocess don't
    /// mix; this proves the model itself builds a well-formed IR that
    /// generation accepts, so the fixture stays covered by the normal
    /// `cargo test --lib` run.
    #[test]
    fn the_fixture_model_generates_without_error() {
        let model = rust_ext_fixture_model();
        let files = generate_target(
            &model,
            TargetKind::Rust,
            &CodegenConfig::default(),
            &rust_casing(),
        )
        .expect("the fixture model must generate cleanly");
        assert!(!files.is_empty());
    }
}

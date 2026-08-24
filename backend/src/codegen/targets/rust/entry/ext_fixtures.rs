//! The IR model the round-trip test (`tests/rust_ext_roundtrip.rs`) compiles
//! against real stand-in crates under `codegen-tests/rust-ext/fixtures/`:
//! a `companyconfig` library (a config load with a per-field
//! `yields`/`returns` projection and a `match`), and its injectable
//! `companybus` handle (an opaque `publisher` type constructed by a free
//! `connect` call, handed by value to a second free `attach` call that
//! builds the `relay` handle the op's own `impl .relay.send(..)` body
//! drives) — the same two libraries the Go and TypeScript fixtures
//! exercise, scoped to this target's own emission.
//!
//! Two shapes here exist to keep the emitter honest about what the frontend
//! really sends. Every per-language `call:` argument that names a logical
//! parameter arrives as `CallArg::Param`, never pre-resolved to a `Ref`,
//! and `load`'s parameters (`svc`/`reg`) deliberately do not share a name
//! with the entry fields feeding them (`service`/`region`), so a leaf that
//! reads a same-named field instead of substituting positionally cannot
//! compile. And `attach` takes the constructed `bus` handle by value: the
//! stand-in `Publisher` is not `Clone`, so a leaf that clones a handle
//! argument out of its stored `Option` slot cannot compile either.

use crate::ir::{
    ArmValue, CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam,
    ForeignField, ForeignLang, ForeignStruct, LangPath, Member, Model, Module, OpImplCall,
    OpaqueType, Prim, ReturnsField, ReturnsLit, ReturnsValue, Select, SelectArm, Shape, ShapeKind,
    Source, Tref, YieldsPos, TONO_IR_VERSION,
};

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

fn ref_to(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
}

fn string_field(name: &str, sources: Vec<Source>) -> EntryField {
    entry_field(name, Tref::Prim(Prim::String), sources)
}

fn entry_field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn member(name: &str) -> Member {
    member_typed(name, Tref::Prim(Prim::String))
}

fn member_typed(name: &str, target: Tref) -> Member {
    Member {
        name: name.into(),
        target,
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

/// A foreign struct whose fields are all `string`, save one that may point
/// at another declared foreign struct (`nested`).
fn foreign_struct(
    name: &str,
    string_fields: &[&str],
    nested: Option<(&str, &str)>,
) -> ForeignStruct {
    let mut fields: Vec<ForeignField> = string_fields
        .iter()
        .map(|f| ForeignField {
            name: (*f).to_string(),
            r#type: Tref::Prim(Prim::String),
        })
        .collect();
    if let Some((field, target_id)) = nested {
        fields.push(ForeignField {
            name: field.into(),
            r#type: ref_to(target_id),
        });
    }
    ForeignStruct {
        name: name.into(),
        langs: vec![],
        fields,
    }
}

fn string_params(names: &[&str]) -> Vec<ExternParam> {
    names
        .iter()
        .map(|n| ExternParam {
            name: (*n).to_string(),
            r#type: Tref::Prim(Prim::String),
        })
        .collect()
}

fn ref_args(names: &[&str]) -> Vec<CallArg> {
    names
        .iter()
        .map(|n| CallArg::Ref(vec![(*n).to_string()]))
        .collect()
}

fn param_args(names: &[&str]) -> Vec<CallArg> {
    names
        .iter()
        .map(|n| CallArg::Param((*n).to_string()))
        .collect()
}

/// A free constructor extern: `name(params) -> ret`, bound to the Rust
/// symbol of the same name, awaited, its `Ok` value assigned whole (no
/// `yields`/`returns`/`errors:`).
fn constructor_extern(name: &str, params: Vec<ExternParam>, ret: Tref) -> ExternDecl {
    let param_names: Vec<&str> = params.iter().map(|p| p.name.as_str()).collect();
    ExternDecl {
        name: name.into(),
        params: params.clone(),
        r#return: ret,
        langs: vec![ExternLang {
            lang: "rust".into(),
            symbol: name.into(),
            call_args: param_args(&param_names),
            yields: vec![],
            returns: None,
            chain: None,
        }],
        r#async: vec!["rust".into()],
        errors: vec![],
    }
}

fn field_path(segments: &[&str]) -> ArmValue {
    ArmValue::Field(strings(segments))
}

/// `companyconfig.load` and the `companybus` handles, combined onto one
/// `client` entry the same way the Go and TypeScript
/// fixtures combine them: an entry `config` field reads `service`/`region`
/// through the free `companyconfig.load` call (whose own parameters are
/// named `svc`/`reg`), the library's returned shape has its `env` matched
/// to pick `host`/`dev_host`, `token` reads `credentials.secret`, an
/// injectable `bus` field is constructed by the free `companybus.connect`
/// call, a `relay` field is constructed by `companybus.attach(.bus,
/// .service)` (the handle moves into the second call), and the `publish`
/// op's own body is `impl .relay.send(topic, payload.body)`.
pub fn rust_ext_fixture_model() -> Model {
    let service = string_field("service", vec![Source::Default(serde_json::json!("notes"))]);
    let region = string_field("region", vec![Source::Arg]);
    let mut config = string_field("config", vec![]);
    config.target = ref_to("m#app_config");
    config.call = Some(EntryCall {
        ns: "companyconfig".into(),
        func: "load".into(),
        args: ref_args(&["service", "region"]),
    });

    let mut bus = entry_field("bus", ref_to("companybus#publisher"), vec![Source::With]);
    bus.call = Some(EntryCall {
        ns: "companybus".into(),
        func: "connect".into(),
        args: vec![
            CallArg::Ref(strings(&["config", "endpoint"])),
            CallArg::Ref(strings(&["config", "token"])),
        ],
    });

    // Injected straight by the caller (`@arg`, no `call` of its own): the
    // gate this exercises is the one a self-constructed handle field
    // (`bus`, above) does not -- a foreign-handle field with no `field.call`
    // resolves to `FieldShape::Json` (its type has no shape in
    // `module.shapes`), not `FieldShape::Scalar`, so it reaches a different
    // leaf of the emitter than `bus`'s own `@with` construction does.
    let hook = entry_field("hook", ref_to("companybus#publisher"), vec![Source::Arg]);

    // A constructed handle handed on to another constructor: `bus` is not
    // `Clone`, so this only compiles if the emitter moves it out of its
    // stored slot rather than cloning it.
    let mut relay = entry_field("relay", ref_to("companybus#relay"), vec![]);
    relay.call = Some(EntryCall {
        ns: "companybus".into(),
        func: "attach".into(),
        args: ref_args(&["bus", "service"]),
    });

    let app_config = Shape {
        id: "m#app_config".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![member("endpoint"), member("token")],
        },
        traits: vec![],
    };

    let note = Shape {
        id: "m#note".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![member("body")],
        },
        traits: vec![],
    };
    let ack = Shape {
        id: "m#ack".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![
                member("id"),
                member_typed("accepted", Tref::Prim(Prim::Bool)),
            ],
        },
        traits: vec![],
    };
    let overloaded = Shape {
        id: "m#overloaded".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![member("message")],
        },
        traits: vec![crate::ir::Trait {
            id: "foreign".into(),
            value: serde_json::json!([
                {"lang": "rust", "name": "BusError::Busy", "fields": {"message": "to_string()"}}
            ]),
        }],
    };

    let publish_op = Shape {
        id: "m#client.publish".into(),
        kind: ShapeKind::Operation {
            input: Some(ref_to("m#note")),
            input_name: Some("payload".into()),
            output: Some(ref_to("m#ack")),
            output_nullable: false,
            errors: vec![ref_to("m#overloaded")],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["relay".into()],
                method: "send".into(),
                args: vec![
                    CallArg::Lit(serde_json::json!("notes")),
                    CallArg::Ref(strings(&["payload", "body"])),
                ],
            }),
        },
        traits: vec![],
    };

    let client = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![service, region, config, bus, hook, relay],
            operations: vec![publish_op],
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
            foreign_struct("Creds", &["secret"], None),
            foreign_struct(
                "Config",
                &["host", "dev_host", "env"],
                Some(("credentials", "companyconfig#Creds")),
            ),
        ],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: string_params(&["svc", "reg"]),
            r#return: ref_to("m#app_config"),
            langs: vec![ExternLang {
                lang: "rust".into(),
                symbol: "load".into(),
                call_args: param_args(&["svc", "reg"]),
                yields: vec![YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(ref_to("companyconfig#Config")),
                    is_error: false,
                    foreign: None,
                }],
                returns: Some(ReturnsLit {
                    r#type: ref_to("m#app_config"),
                    fields: vec![
                        ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Select(Select {
                                subject: strings(&["cfg", "env"]),
                                subject_index: None,
                                arms: vec![
                                    SelectArm {
                                        pattern: Some(serde_json::json!("prod")),
                                        value: field_path(&["cfg", "host"]),
                                    },
                                    SelectArm {
                                        pattern: None,
                                        value: field_path(&["cfg", "dev_host"]),
                                    },
                                ],
                            }),
                        },
                        ReturnsField {
                            name: "token".into(),
                            value: ReturnsValue::Field(strings(&["cfg", "credentials", "secret"])),
                        },
                    ],
                }),
                chain: None,
            }],
            r#async: vec!["rust".into()],
            errors: vec![],
        }],
    };

    let bus_lib = ExtLib {
        name: "companybus".into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "companybus".into(),
        }],
        structs: vec![ForeignStruct {
            name: "Ack".into(),
            fields: vec![
                ForeignField {
                    name: "id".into(),
                    r#type: Tref::Prim(Prim::String),
                },
                ForeignField {
                    name: "accepted".into(),
                    r#type: Tref::Prim(Prim::Bool),
                },
            ],
            langs: vec![],
        }],
        types: vec![
            OpaqueType {
                name: "publisher".into(),
                langs: vec![ForeignLang {
                    lang: "rust".into(),
                    name: "Publisher".into(),
                    fields: Default::default(),
                }],
                methods: vec![],
            },
            OpaqueType {
                name: "relay".into(),
                langs: vec![ForeignLang {
                    lang: "rust".into(),
                    name: "Relay".into(),
                    fields: Default::default(),
                }],
                methods: vec![ExternDecl {
                    name: "send".into(),
                    params: string_params(&["topic", "body"]),
                    r#return: ref_to("m#ack"),
                    langs: vec![ExternLang {
                        lang: "rust".into(),
                        symbol: "send".into(),
                        call_args: param_args(&["topic", "body"]),
                        yields: vec![YieldsPos {
                            name: "a".into(),
                            r#type: Some(ref_to("companybus#Ack")),
                            is_error: false,
                            foreign: None,
                        }],
                        returns: Some(ReturnsLit {
                            r#type: ref_to("m#ack"),
                            fields: vec![
                                ReturnsField {
                                    name: "id".into(),
                                    value: ReturnsValue::Field(strings(&["a", "id"])),
                                },
                                ReturnsField {
                                    name: "accepted".into(),
                                    value: ReturnsValue::Field(strings(&["a", "accepted"])),
                                },
                            ],
                        }),
                        chain: None,
                    }],
                    r#async: vec!["rust".into()],
                    errors: vec!["m#overloaded".into()],
                }],
            },
        ],
        externs: vec![
            constructor_extern(
                "connect",
                string_params(&["endpoint", "token"]),
                ref_to("companybus#publisher"),
            ),
            constructor_extern(
                "attach",
                vec![
                    ExternParam {
                        name: "source".into(),
                        r#type: ref_to("companybus#publisher"),
                    },
                    ExternParam {
                        name: "tag".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                ],
                ref_to("companybus#relay"),
            ),
        ],
    };

    let module = Module {
        name: "m".into(),
        shapes: vec![client, app_config, note, ack, overloaded],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![ext_lib, bus_lib],
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

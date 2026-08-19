//! IR builders shared by every `ext`/`extern` FFI test fixture: the
//! Go emitter's own unit tests (`ext_tests`, `#[cfg(test)]`) and the
//! `go_ext_roundtrip` integration test (a separate binary, linked against
//! this crate's public API rather than compiled with it). Not `cfg(test)`
//! for that reason: an integration test cannot see anything gated that way,
//! so this stays a small, always-compiled, always-public module rather than
//! source the same builders twice.

use std::collections::BTreeMap;

use crate::ir::{
    ArmValue, CallArg, Empty, EntryCall, EntryField, ErrorBinding, ExtLib, ExternDecl, ExternLang,
    ExternParam, ExternStub, ExternStubTarget, ForeignField, ForeignStruct, LangPath, Member,
    Model, Module, OpImplCall, OpaqueType, Prim, ReturnsField, ReturnsLit, ReturnsValue, Select,
    SelectArm, Shape, ShapeKind, Source, StubAnswer, TestCall, TestConstruction, TestDecl,
    TestExpect, TestPattern, Trait, Tref, YieldsPos, TONO_IR_VERSION,
};

pub fn string_t() -> Tref {
    Tref::Prim(Prim::String)
}

pub fn member(name: &str, target: Tref, required: bool) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

pub fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

pub fn field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
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

pub fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

pub fn ext_param(name: &str, target: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: target,
    }
}

/// A single-language (`go`) extern declaration: the shape every `extern` in
/// these fixtures shares (one `ExternLang`, no per-language variation), so a
/// call site only spells what actually differs between externs (name,
/// params, symbol, yields, returns, errors) instead of repeating the whole
/// `ExternDecl { .. langs: vec![ExternLang { .. }] }` skeleton each time.
#[allow(clippy::too_many_arguments)]
pub fn go_extern(
    name: &str,
    params: Vec<ExternParam>,
    ret: Tref,
    symbol: &str,
    call_args: Vec<CallArg>,
    yields: Vec<YieldsPos>,
    returns: Option<ReturnsLit>,
    errors: Vec<ErrorBinding>,
) -> ExternDecl {
    ExternDecl {
        name: name.into(),
        params,
        r#return: ret,
        langs: vec![ExternLang {
            lang: "go".into(),
            symbol: symbol.into(),
            call_args,
            yields,
            returns,
            errors,
            sync: false,
            infallible: false,
            ctx: false,
        }],
    }
}

/// The same shape as [`go_extern`], marked `infallible`: a foreign function
/// with no error return, like `uuid.NewString() string`.
#[allow(clippy::too_many_arguments)]
pub fn go_extern_infallible(
    name: &str,
    params: Vec<ExternParam>,
    ret: Tref,
    symbol: &str,
    call_args: Vec<CallArg>,
    yields: Vec<YieldsPos>,
    returns: Option<ReturnsLit>,
) -> ExternDecl {
    let mut decl = go_extern(
        name,
        params,
        ret,
        symbol,
        call_args,
        yields,
        returns,
        vec![],
    );
    decl.langs[0].infallible = true;
    decl
}

/// The same shape as [`go_extern`], marked `ctx`: a foreign handle's own
/// method call that receives the target's cancellation/deadline context in
/// its idiomatic position (Go: `ctx context.Context` as the first
/// parameter).
#[allow(clippy::too_many_arguments)]
pub fn go_extern_ctx(
    name: &str,
    params: Vec<ExternParam>,
    ret: Tref,
    symbol: &str,
    call_args: Vec<CallArg>,
    yields: Vec<YieldsPos>,
    returns: Option<ReturnsLit>,
    errors: Vec<ErrorBinding>,
) -> ExternDecl {
    let mut decl = go_extern(
        name, params, ret, symbol, call_args, yields, returns, errors,
    );
    decl.langs[0].ctx = true;
    decl
}

/// An `ext` block declaring only a Go module path, for the common case
/// (these fixtures never exercise a lib bound for more than one target).
pub fn go_ext_lib(
    name: &str,
    path: &str,
    structs: Vec<ForeignStruct>,
    types: Vec<OpaqueType>,
    externs: Vec<ExternDecl>,
) -> ExtLib {
    ExtLib {
        name: name.into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: path.into(),
        }],
        structs,
        types,
        externs,
    }
}

/// A worked example of the `ext`/`extern` FFI surface, trimmed to the Go
/// constructs these fixtures cover: a field-construction call with a
/// `match` projection, an injectable handle with a construction fallback,
/// and an op-level method-call form with a declared sentinel. An HTTP op
/// alongside these is deliberately left out: it would exercise the
/// (already covered) transport machinery, not the ext/extern surface these
/// fixtures target.
///
/// Shared by the Go emitter's own `ext_tests` (a Rust-level check of the
/// generated statements) and the `go_ext_roundtrip` integration test (a
/// `go build` proof against real stand-in libraries under
/// `codegen-tests/go-ext/fixtures/`): both need exactly this shape, so one
/// definition serves both instead of two near-identical copies drifting
/// apart.
pub fn reference_example_module() -> Module {
    let app_config = structure(
        "m#app_config",
        vec![
            member("endpoint", string_t(), true),
            member("token", string_t(), true),
        ],
    );
    let note = structure(
        "m#note",
        vec![
            member("id", string_t(), true),
            member("body", string_t(), true),
        ],
    );
    let ack = structure(
        "m#ack",
        vec![
            member("id", string_t(), true),
            member("accepted", Tref::Prim(Prim::Bool), true),
        ],
    );
    let mut overloaded = structure("m#overloaded", vec![member("message", string_t(), true)]);
    overloaded.traits = vec![Trait {
        id: "retryable".into(),
        value: serde_json::Value::Null,
    }];

    let config_field = {
        let mut f = field(
            "config",
            Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            vec![],
        );
        f.call = Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![call_ref(&["service"]), call_ref(&["region"])],
        });
        f
    };
    let bus_field = {
        let mut f = field(
            "bus",
            Tref::Ref {
                id: "companybus#publisher".into(),
                args: vec![],
            },
            vec![Source::With],
        );
        f.call = Some(EntryCall {
            ns: "companybus".into(),
            func: "connect".into(),
            args: vec![
                call_ref(&["config", "endpoint"]),
                call_ref(&["config", "token"]),
            ],
        });
        f
    };
    let service_field = field(
        "service",
        string_t(),
        vec![Source::Default(serde_json::json!("notes"))],
    );
    let region_field = field("region", string_t(), vec![Source::Arg]);

    let publish_op = Shape {
        id: "m#client.publish".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            input_name: Some("payload".into()),
            output: Some(Tref::Ref {
                id: "m#ack".into(),
                args: vec![],
            }),
            errors: vec![Tref::Ref {
                id: "m#overloaded".into(),
                args: vec![],
            }],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["bus".into()],
                method: "send".into(),
                args: vec![
                    CallArg::Lit(serde_json::json!("notes")),
                    call_ref(&["payload", "body"]),
                ],
            }),
        },
        traits: vec![],
    };

    let entry = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![service_field, region_field, config_field, bus_field],
            operations: vec![publish_op],
        },
        traits: vec![],
    };

    let companyconfig = go_ext_lib(
        "companyconfig",
        "tono-ext-fixture/companyconfig",
        vec![
            ForeignStruct {
                name: "go_config".into(),
                fields: vec![
                    ForeignField {
                        name: "Host".into(),
                        r#type: string_t(),
                    },
                    ForeignField {
                        name: "DevHost".into(),
                        r#type: string_t(),
                    },
                    ForeignField {
                        name: "Env".into(),
                        r#type: string_t(),
                    },
                    ForeignField {
                        name: "Credentials".into(),
                        r#type: Tref::Ref {
                            id: "companyconfig#go_creds".into(),
                            args: vec![],
                        },
                    },
                ],
            },
            ForeignStruct {
                name: "go_creds".into(),
                fields: vec![ForeignField {
                    name: "Secret".into(),
                    r#type: string_t(),
                }],
            },
        ],
        vec![],
        vec![go_extern(
            "load",
            vec![
                ext_param("service", string_t()),
                ext_param("region", string_t()),
            ],
            Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            "Load",
            vec![
                CallArg::Param("service".into()),
                CallArg::Param("region".into()),
            ],
            vec![YieldsPos {
                name: "cfg".into(),
                r#type: Some(Tref::Ref {
                    id: "companyconfig#go_config".into(),
                    args: vec![],
                }),
                is_error: false,
            }],
            Some(ReturnsLit {
                r#type: Tref::Ref {
                    id: "m#app_config".into(),
                    args: vec![],
                },
                fields: vec![
                    ReturnsField {
                        name: "endpoint".into(),
                        value: ReturnsValue::Select(Select {
                            subject: vec!["cfg".into(), "Env".into()],
                            subject_index: None,
                            arms: vec![
                                SelectArm {
                                    pattern: Some(serde_json::json!("prod")),
                                    value: ArmValue::Field(vec!["cfg".into(), "Host".into()]),
                                },
                                SelectArm {
                                    pattern: None,
                                    value: ArmValue::Field(vec!["cfg".into(), "DevHost".into()]),
                                },
                            ],
                        }),
                    },
                    ReturnsField {
                        name: "token".into(),
                        value: ReturnsValue::Field(vec![
                            "cfg".into(),
                            "Credentials".into(),
                            "Secret".into(),
                        ]),
                    },
                ],
            }),
            vec![],
        )],
    );

    let companybus = go_ext_lib(
        "companybus",
        "tono-ext-fixture/companybus",
        vec![ForeignStruct {
            name: "go_ack".into(),
            fields: vec![
                ForeignField {
                    name: "ID".into(),
                    r#type: string_t(),
                },
                ForeignField {
                    name: "OK".into(),
                    r#type: Tref::Prim(Prim::Bool),
                },
            ],
        }],
        vec![OpaqueType {
            name: "publisher".into(),
            instance: None,
            methods: vec![go_extern(
                "send",
                vec![
                    ext_param("topic", string_t()),
                    ext_param("body", string_t()),
                ],
                Tref::Ref {
                    id: "m#ack".into(),
                    args: vec![],
                },
                "Send",
                vec![
                    CallArg::Param("topic".into()),
                    CallArg::Param("body".into()),
                ],
                vec![YieldsPos {
                    name: "a".into(),
                    r#type: Some(Tref::Ref {
                        id: "companybus#go_ack".into(),
                        args: vec![],
                    }),
                    is_error: false,
                }],
                Some(ReturnsLit {
                    r#type: Tref::Ref {
                        id: "m#ack".into(),
                        args: vec![],
                    },
                    fields: vec![
                        ReturnsField {
                            name: "id".into(),
                            value: ReturnsValue::Field(vec!["a".into(), "ID".into()]),
                        },
                        ReturnsField {
                            name: "accepted".into(),
                            value: ReturnsValue::Field(vec!["a".into(), "OK".into()]),
                        },
                    ],
                }),
                vec![ErrorBinding {
                    sentinel: "ErrBusy".into(),
                    r#type: "overloaded".into(),
                }],
            )],
        }],
        vec![go_extern(
            "connect",
            vec![
                ext_param("endpoint", string_t()),
                ext_param("token", string_t()),
            ],
            Tref::Ref {
                id: "companybus#publisher".into(),
                args: vec![],
            },
            "Connect",
            vec![
                CallArg::Param("endpoint".into()),
                CallArg::Param("token".into()),
            ],
            vec![],
            None,
            vec![],
        )],
    );

    Module {
        name: "m".into(),
        shapes: vec![app_config, note, ack, overloaded, entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![companyconfig, companybus],
        tests: reference_example_tests(),
    }
}

/// The free-fn stub every test constructing `client` needs: `bus`'s own
/// construction call (`companybus.connect`) is reachable regardless of
/// whether a test also calls `publish`, since `reachable_externs` walks
/// every field's `call` unconditionally. The answer itself is never decoded
/// (a handle-typed free stub only marks the field overridden; the actual
/// fake implementing the handle's interface comes from a handle-method
/// stub), so its value is a placeholder.
fn bus_connect_stub() -> ExternStub {
    ExternStub {
        binding: None,
        target: ExternStubTarget::Free {
            lib: "companybus".into(),
            fn_: "connect".into(),
        },
        answers: vec![StubAnswer::Value {
            value: serde_json::Value::Null,
        }],
    }
}

fn config_load_stub() -> ExternStub {
    ExternStub {
        binding: None,
        target: ExternStubTarget::Free {
            lib: "companyconfig".into(),
            fn_: "load".into(),
        },
        answers: vec![StubAnswer::Value {
            value: serde_json::json!({"endpoint": "https://api.example.com", "token": "s3cr3t"}),
        }],
    }
}

/// The two declared tests the appendix fixture itself specifies: a
/// construction-only test stubbing the free `companyconfig.load` call, and a
/// call test additionally stubbing the `companybus.publisher.send` handle
/// method the `publish` op's own `impl` body reaches. Both are hermetic: no
/// stub answer ever calls into the real `companyconfig`/`companybus`
/// libraries.
fn reference_example_tests() -> Vec<TestDecl> {
    let construction = TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([("region".to_string(), serde_json::json!("us-east"))]),
    };
    let construction_only = TestDecl {
        name: "constructs the client from the stubbed config".into(),
        constructions: vec![construction.clone()],
        stubs: vec![],
        extern_stubs: vec![config_load_stub(), bus_connect_stub()],
        calls: vec![],
        expects: vec![TestExpect::Outcome {
            subject: "c".into(),
            pattern: TestPattern::Ok(Empty {}),
        }],
    };
    let publishes = TestDecl {
        name: "publishes a note through the stubbed handle".into(),
        constructions: vec![construction],
        stubs: vec![],
        extern_stubs: vec![
            config_load_stub(),
            bus_connect_stub(),
            ExternStub {
                binding: None,
                target: ExternStubTarget::Method {
                    lib: "companybus".into(),
                    ty: "publisher".into(),
                    method: "send".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::json!({"id": "n1", "accepted": true}),
                }],
            },
        ],
        calls: vec![TestCall {
            binding: "r".into(),
            client: "c".into(),
            op: "publish".into(),
            input: Some(serde_json::json!({"id": "n1", "body": "hello"})),
        }],
        expects: vec![TestExpect::Outcome {
            subject: "r".into(),
            pattern: TestPattern::Ok(Empty {}),
        }],
    };
    vec![construction_only, publishes]
}

/// [`reference_example_module`], wrapped in a `Model`.
pub fn reference_example_model() -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![reference_example_module()],
    }
}

/// A minimal repro of a real single-return Go function bound as `extern`:
/// `idgen.NewString() string` (no error, matching `uuid.NewString`), marked
/// `infallible`, with no `yields:` at all (the zero-yields path is one of
/// the two shapes the Go emitter used to always destructure as two values
/// regardless of the real function's own arity).
pub fn infallible_extern_module() -> Module {
    let id_field = {
        let mut f = field("id", string_t(), vec![]);
        f.call = Some(EntryCall {
            ns: "idgen".into(),
            func: "new_id".into(),
            args: vec![],
        });
        f
    };
    let entry = Shape {
        id: "n#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![id_field],
            operations: vec![],
        },
        traits: vec![],
    };
    let idgen = go_ext_lib(
        "idgen",
        "tono-ext-fixture/idgen",
        vec![],
        vec![],
        vec![go_extern_infallible(
            "new_id",
            vec![],
            string_t(),
            "NewString",
            vec![],
            vec![],
            None,
        )],
    );
    Module {
        name: "n".into(),
        shapes: vec![entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![idgen],
        tests: vec![],
    }
}

/// [`infallible_extern_module`], wrapped in a `Model`.
pub fn infallible_extern_model() -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![infallible_extern_module()],
    }
}

/// A minimal repro of the `ctx` marker: a foreign handle with one `ctx`-
/// marked method, reached through an op's own `impl .conn.get(args)` body
/// (the one construct the marker is legal on). The handle's own construction
/// call is a plain, unmarked free extern, matching the frontend gate that
/// only a handle method may carry `ctx`.
pub fn ctx_extern_module() -> Module {
    let conn_field = {
        let mut f = field(
            "conn",
            Tref::Ref {
                id: "svc#conn".into(),
                args: vec![],
            },
            vec![],
        );
        f.call = Some(EntryCall {
            ns: "svc".into(),
            func: "connect".into(),
            args: vec![],
        });
        f
    };
    let get_op = Shape {
        id: "c#client.get".into(),
        kind: ShapeKind::Operation {
            input: Some(string_t()),
            input_name: Some("id".into()),
            output: Some(string_t()),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["conn".into()],
                method: "get".into(),
                args: vec![call_ref(&["id"])],
            }),
        },
        traits: vec![],
    };
    let entry = Shape {
        id: "c#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![conn_field],
            operations: vec![get_op],
        },
        traits: vec![],
    };
    let svc = go_ext_lib(
        "svc",
        "tono-ext-fixture/svc",
        vec![],
        vec![OpaqueType {
            name: "conn".into(),
            instance: None,
            methods: vec![go_extern_ctx(
                "get",
                vec![ext_param("id", string_t())],
                string_t(),
                "Get",
                vec![CallArg::Param("id".into())],
                vec![],
                None,
                vec![],
            )],
        }],
        vec![go_extern(
            "connect",
            vec![],
            Tref::Ref {
                id: "svc#conn".into(),
                args: vec![],
            },
            "Connect",
            vec![],
            vec![],
            None,
            vec![],
        )],
    );
    Module {
        name: "c".into(),
        shapes: vec![entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![svc],
        tests: vec![],
    }
}

/// [`ctx_extern_module`], wrapped in a `Model`.
pub fn ctx_extern_model() -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![ctx_extern_module()],
    }
}

// Split out to keep this file under the file-size ceiling.
#[path = "ext_fixtures_compose.rs"]
mod ext_fixtures_compose;
pub use ext_fixtures_compose::{composed_handles_model, composed_handles_module};

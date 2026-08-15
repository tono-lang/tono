//! End-to-end check that the Go the engine emits for the `ext`/`extern` FFI
//! library block (RFC-0023) compiles against a real library: the module in
//! this test mirrors the RFC's own appendix (a config load with a per-field
//! `yields`/`returns` projection and a `match`, an injectable bus handle
//! with a construction fallback, and an op implemented by a call into the
//! handle's own method with a declared sentinel-to-error mapping).
//!
//! The verification model RFC-0023 asks for is the target compiler, not a
//! Rust assertion: the generated Go reads `cfg.Host`/`ack.OK` directly off
//! whatever the stand-in libraries under `fixtures/` return, so a
//! `fixtures/` package that does not match the declared shape must fail
//! `go build`, not this harness. The negative test below proves that.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Both tests write into the same `codegen-tests/go-ext/` tree (the sdk
/// output, and — for the negative test — the fixture library itself), so
/// they cannot run concurrently.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

use tono_backend::codegen::modules::CodegenConfig;
use tono_backend::codegen::pipeline::generate_target;
use tono_backend::codegen::targets::go::types::go_casing;
use tono_backend::codegen::{Formatter, TargetKind};
use tono_backend::ir::*;

fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/go-ext")
}

fn member(name: &str, target: Tref) -> Member {
    Member {
        name: name.into(),
        target,
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

fn field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
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

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

fn string() -> Tref {
    Tref::Prim(Prim::String)
}

fn ext_param(name: &str, target: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: target,
    }
}

/// A single-language (`go`) extern declaration: the shape every `extern` in
/// this fixture shares (one `ExternLang`, no per-language variation), so a
/// call site only spells what actually differs between `load`/`send`/
/// `connect` instead of the whole `ExternDecl { .. langs: vec![ExternLang {
/// .. }] }` skeleton each time.
#[allow(clippy::too_many_arguments)]
fn go_extern(
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
        }],
    }
}

/// An `ext` block declaring only a Go module path, for the common case (this
/// fixture never exercises a lib bound for more than one target).
fn go_ext_lib(
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

/// The IR model the RFC-0023 appendix describes, trimmed to the constructs
/// this task covers (the field-construction call with a `match` projection,
/// the injectable handle with a construction fallback, and the op-level
/// method-call form with a declared sentinel). The HTTP `fetch` op from the
/// appendix is left out: it exercises the (already covered) transport
/// machinery, not the ext/extern surface this test targets.
fn model() -> Model {
    let app_config = structure(
        "m#app_config",
        vec![member("endpoint", string()), member("token", string())],
    );
    let note = structure(
        "m#note",
        vec![member("id", string()), member("body", string())],
    );
    let ack = structure(
        "m#ack",
        vec![
            member("id", string()),
            member("accepted", Tref::Prim(Prim::Bool)),
        ],
    );
    let mut overloaded = structure("m#overloaded", vec![member("message", string())]);
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
        string(),
        vec![Source::Default(serde_json::json!("notes"))],
    );
    let region_field = field("region", string(), vec![Source::Arg]);

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
                        r#type: string(),
                    },
                    ForeignField {
                        name: "DevHost".into(),
                        r#type: string(),
                    },
                    ForeignField {
                        name: "Env".into(),
                        r#type: string(),
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
                    r#type: string(),
                }],
            },
        ],
        vec![],
        vec![go_extern(
            "load",
            vec![
                ext_param("service", string()),
                ext_param("region", string()),
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
                    r#type: string(),
                },
                ForeignField {
                    name: "OK".into(),
                    r#type: Tref::Prim(Prim::Bool),
                },
            ],
        }],
        vec![OpaqueType {
            name: "publisher".into(),
            methods: vec![go_extern(
                "send",
                vec![ext_param("topic", string()), ext_param("body", string())],
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
                ext_param("endpoint", string()),
                ext_param("token", string()),
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

    let module = Module {
        name: "m".into(),
        shapes: vec![app_config, note, ack, overloaded, entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![companyconfig, companybus],
        tests: vec![],
    };

    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    }
}

/// Generate the model for Go, gofmt every file, and write it under
/// `codegen-tests/go-ext/sdk/`, alongside a `go.mod` pointing the two `ext`
/// library import paths at the stand-in packages under `fixtures/` (via
/// `replace`, the same mechanism a real consumer's own `go.mod` would use
/// for a private/internal dependency).
fn write_sdk(model: &Model) -> PathBuf {
    let dir = fixtures_dir().join("sdk");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create sdk dir");

    let config = CodegenConfig {
        flatten: true,
        remap: vec![],
        go_module: None,
    };
    let casing = go_casing();
    let files = generate_target(model, TargetKind::Go, &config, &casing)
        .expect("generate_target(Go) must succeed for a well-formed ext model");
    assert!(!files.is_empty(), "expected generated Go files");

    for file in &files {
        let formatted = Formatter::new("gofmt", vec![]).run(&file.text);
        assert!(
            formatted.warning.is_none(),
            "gofmt must format {} cleanly: {:?}\n{}",
            file.path.display(),
            formatted.warning,
            file.text
        );
        let out = dir.join(&file.path);
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, formatted.text).unwrap();
    }

    std::fs::write(
        dir.join("go.mod"),
        "module tono-ext-fixture/sdk\n\n\
         go 1.21\n\n\
         require tono-ext-fixture/companyconfig v0.0.0\n\
         require tono-ext-fixture/companybus v0.0.0\n\n\
         replace tono-ext-fixture/companyconfig => ../fixtures/companyconfig\n\
         replace tono-ext-fixture/companybus => ../fixtures/companybus\n",
    )
    .unwrap();
    dir
}

#[test]
fn the_rfc_appendix_generates_go_that_compiles_against_the_real_libraries() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&model());
    let build = Command::new("go")
        .arg("build")
        .arg("./...")
        .current_dir(&dir)
        .output()
        .expect("run go build");
    assert!(
        build.status.success(),
        "generated Go failed to build:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// Declaring a field the library does not actually have (or reordering an
/// argument list with incompatible types) must break the Go build: the
/// declaration is a hypothesis the target compiler grades (RFC-0023, degrau
/// 2), not a contract `tono` itself confirms. This renames the real
/// `companyconfig.Config.Host` field so the generated `cfg.Host` access no
/// longer compiles, without touching the generator at all.
#[test]
fn a_field_the_library_does_not_have_breaks_the_go_build() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&model());
    let config_go = fixtures_dir().join("fixtures/companyconfig/config.go");
    let original = std::fs::read_to_string(&config_go).unwrap();
    let broken = original.replace("Host", "Address");
    std::fs::write(&config_go, &broken).unwrap();
    let result = Command::new("go")
        .arg("build")
        .arg("./...")
        .current_dir(&dir)
        .output()
        .expect("run go build");
    std::fs::write(&config_go, &original).unwrap();
    assert!(
        !result.status.success(),
        "expected the build to fail once the library no longer has the declared field"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("Host"),
        "expected the compiler error to name the missing field:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

//! End-to-end check that the TypeScript the engine emits for the
//! `ext`/`extern` FFI library block compiles against a real library: the
//! module in this test exercises a construction-only worked example (a
//! config load with a `Ctor` argument and a `yields`/`returns` projection,
//! an injectable bus handle with a construction fallback, and a declared
//! sentinel-to-error mapping).
//!
//! The verification model this test relies on is the target compiler, not a
//! Rust assertion: the generated TypeScript reads `raw.host`/`raw.token`
//! directly off whatever the stand-in libraries under `fixtures/` return, so
//! a `fixtures/` module that does not match the declared shape must fail
//! `tsc`, not this harness. The negative test below proves that.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use tono_backend::codegen::modules::CodegenConfig;
use tono_backend::codegen::pipeline::generate_target;
use tono_backend::codegen::targets::typescript::types::ts_casing;
use tono_backend::codegen::{Formatter, TargetKind};
use tono_backend::ir::{
    CallArg, CallCtor, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam,
    ForeignField, ForeignStruct, LangPath, Model, Module, OpaqueType, Prim, ReturnsField,
    ReturnsLit, ReturnsValue, Shape, ShapeKind, Source, Tref, YieldsPos, TONO_IR_VERSION,
};

/// Both tests write into the same `codegen-tests/ts-ext/` tree (the sdk
/// output, and, for the negative test, the fixture library itself), so they
/// cannot run concurrently.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/ts-ext")
}

fn ef(name: &str, target: Tref, sources: Vec<Source>, call: Option<EntryCall>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn string_field(name: &str) -> ForeignField {
    ForeignField {
        name: name.into(),
        r#type: Tref::Prim(Prim::String),
    }
}

/// The appendix worked example: `companyconfig.load` projects a foreign
/// `Ctor` argument and a `yields`/`returns` pair with a declared sentinel,
/// `companybus.connect` constructs an injectable opaque handle with no
/// projection at all (its raw result already is the logical value), and one
/// plain wire operation reads the resolved config back to prove construction
/// and the ordinary HTTP surface still compose. The bus handle's own
/// `send` method is not exercised by an operation: no target's codegen
/// consumes an op's own `impl .field.method(..)` body yet.
fn appendix_model() -> Model {
    let mut load_ctor_fields = std::collections::BTreeMap::new();
    load_ctor_fields.insert("region".to_string(), CallArg::Param("region".into()));
    load_ctor_fields.insert("service".to_string(), CallArg::Param("service".into()));
    let companyconfig = ExtLib {
        name: "companyconfig".into(),
        langs: vec![LangPath {
            lang: "ts".into(),
            path: "@company/config".into(),
        }],
        structs: vec![
            ForeignStruct {
                name: "ts_opts".into(),
                fields: vec![string_field("region"), string_field("service")],
            },
            ForeignStruct {
                name: "ts_config".into(),
                fields: vec![string_field("host"), string_field("token")],
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
                id: "main#app_config".into(),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "ts".into(),
                symbol: "load".into(),
                call_args: vec![CallArg::Ctor(CallCtor {
                    name: "ts_opts".into(),
                    fields: load_ctor_fields,
                })],
                yields: vec![YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(Tref::Ref {
                        id: "companyconfig#ts_config".into(),
                        args: vec![],
                    }),
                    is_error: false,
                }],
                returns: Some(ReturnsLit {
                    r#type: Tref::Ref {
                        id: "main#app_config".into(),
                        args: vec![],
                    },
                    fields: vec![
                        ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Field(vec!["cfg".into(), "host".into()]),
                        },
                        ReturnsField {
                            name: "token".into(),
                            value: ReturnsValue::Field(vec!["cfg".into(), "token".into()]),
                        },
                    ],
                }),
                errors: vec![tono_backend::ir::ErrorBinding {
                    sentinel: "BUSY".into(),
                    r#type: "overloaded".into(),
                }],
            }],
        }],
    };
    let companybus = ExtLib {
        name: "companybus".into(),
        langs: vec![LangPath {
            lang: "ts".into(),
            path: "@company/bus".into(),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: "publisher".into(),
            methods: vec![],
        }],
        externs: vec![ExternDecl {
            name: "connect".into(),
            params: vec![
                ExternParam {
                    name: "endpoint".into(),
                    r#type: Tref::Prim(Prim::String),
                },
                ExternParam {
                    name: "token".into(),
                    r#type: Tref::Prim(Prim::String),
                },
            ],
            r#return: Tref::Ref {
                id: "companybus#publisher".into(),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "ts".into(),
                symbol: "connect".into(),
                call_args: vec![
                    CallArg::Param("endpoint".into()),
                    CallArg::Param("token".into()),
                ],
                yields: vec![],
                returns: None,
                errors: vec![],
            }],
        }],
    };

    let service = ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None);
    let region = ef("region", Tref::Prim(Prim::String), vec![Source::Arg], None);
    let config = ef(
        "config",
        Tref::Ref {
            id: "main#app_config".into(),
            args: vec![],
        },
        vec![],
        Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![
                CallArg::Ref(vec!["service".into()]),
                CallArg::Ref(vec!["region".into()]),
            ],
        }),
    );
    let mut bus = ef(
        "bus",
        Tref::Ref {
            id: "companybus#publisher".into(),
            args: vec![],
        },
        vec![Source::With],
        Some(EntryCall {
            ns: "companybus".into(),
            func: "connect".into(),
            args: vec![
                CallArg::Ref(vec!["config".into(), "endpoint".into()]),
                CallArg::Ref(vec!["config".into(), "token".into()]),
            ],
        }),
    );
    bus.sources = vec![Source::With];

    let app_config = Shape {
        id: "main#app_config".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![
                tono_backend::ir::Member {
                    name: "endpoint".into(),
                    target: Tref::Prim(Prim::String),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                },
                tono_backend::ir::Member {
                    name: "token".into(),
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
    let note_ref = Shape {
        id: "main#note_ref".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![tono_backend::ir::Member {
                name: "id".into(),
                target: Tref::Prim(Prim::String),
                required: true,
                default: None,
                constraints: vec![],
                traits: vec![],
            }],
        },
        traits: vec![],
    };
    let note = Shape {
        id: "main#note".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![tono_backend::ir::Member {
                name: "id".into(),
                target: Tref::Prim(Prim::String),
                required: true,
                default: None,
                constraints: vec![],
                traits: vec![],
            }],
        },
        traits: vec![],
    };
    // The one wire operation any real entry declares alongside its
    // construction fields, so the round-trip exercises the same shape a
    // hand-written spec would (a construction-only entry with no operation
    // at all is not how this construct is actually used).
    let fetch = Shape {
        id: "main#client.fetch".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "main#note_ref".into(),
                args: vec![],
            }),
            input_name: Some("ref".into()),
            output: Some(Tref::Ref {
                id: "main#note".into(),
                args: vec![],
            }),
            errors: vec![],
            wire: Some(Box::new(tono_backend::ir::WireBinding {
                method: "GET".into(),
                uri: tono_backend::ir::WireValue::Template(vec![
                    tono_backend::ir::TemplatePart::Lit("/notes/".into()),
                    tono_backend::ir::TemplatePart::Input("id".into()),
                ]),
                body: None,
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: Some(tono_backend::ir::WireValue::Field(vec![
                    "config".into(),
                    "endpoint".into(),
                ])),
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
            impl_call: None,
        },
        traits: vec![],
    };
    let client = Shape {
        id: "main#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![service, region, config, bus],
            operations: vec![fetch],
        },
        traits: vec![],
    };
    let module = Module {
        tests: vec![],
        name: "main".into(),
        shapes: vec![app_config, note_ref, note, client],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![companyconfig, companybus],
    };
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    }
}

/// Generate the model for TypeScript, prettier-format every file (falling
/// back to the raw text when prettier is not installed, same as the CLI
/// does), and write it under `codegen-tests/ts-ext/sdk/`, alongside a
/// `tsconfig.json` that maps the two `ext` library import paths straight at
/// the stand-in modules under `fixtures/` (via `paths`, so the check needs
/// no package manager or network access at all).
fn write_sdk(model: &Model) -> PathBuf {
    let dir = fixtures_dir().join("sdk");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create sdk dir");

    let config = CodegenConfig {
        flatten: true,
        remap: vec![],
        go_module: None,
    };
    let casing = ts_casing();
    let files = generate_target(model, TargetKind::TypeScript, &config, &casing)
        .expect("generate_target(TypeScript) must succeed for a well-formed ext model");
    assert!(!files.is_empty(), "expected generated TypeScript files");

    for file in &files {
        let formatted = Formatter::new("prettier", vec!["--parser".into(), "typescript".into()])
            .run(&file.text);
        let out = dir.join(&file.path);
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, formatted.text).unwrap();
    }

    std::fs::write(
        dir.join("tsconfig.json"),
        r#"{
  "compilerOptions": {
    "target": "ES2020",
    "module": "ES2020",
    "moduleResolution": "bundler",
    "strict": true,
    "noEmit": true,
    "esModuleInterop": true,
    "paths": {
      "@company/config": ["../fixtures/company-config/index.ts"],
      "@company/bus": ["../fixtures/company-bus/index.ts"]
    }
  },
  "include": ["**/*.ts"]
}
"#,
    )
    .unwrap();
    dir
}

#[test]
fn the_rfc_appendix_generates_typescript_that_compiles_against_the_real_libraries() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test ts_ext_roundtrip`");
        return;
    }
    if !have("tsc", "--version") {
        eprintln!("skipping: TypeScript toolchain (tsc) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&appendix_model());
    let build = Command::new("tsc")
        .arg("-p")
        .arg("tsconfig.json")
        .current_dir(&dir)
        .output()
        .expect("run tsc");
    assert!(
        build.status.success(),
        "generated TypeScript failed to type-check:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// Declaring a field the library does not actually have must break the
/// `tsc` check: the declaration is a hypothesis the target compiler grades,
/// not a contract `tono` itself confirms. This renames the real
/// `company-config` module's `host` field so the generated `raw.host`
/// access no longer type-checks, without touching the generator at all.
#[test]
fn a_field_the_library_does_not_have_breaks_the_typescript_check() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test ts_ext_roundtrip`");
        return;
    }
    if !have("tsc", "--version") {
        eprintln!("skipping: TypeScript toolchain (tsc) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&appendix_model());
    let config_ts = fixtures_dir().join("fixtures/company-config/index.ts");
    let original = std::fs::read_to_string(&config_ts).unwrap();
    let broken = original.replace("host", "address");
    std::fs::write(&config_ts, &broken).unwrap();
    let result = Command::new("tsc")
        .arg("-p")
        .arg("tsconfig.json")
        .current_dir(&dir)
        .output()
        .expect("run tsc");
    std::fs::write(&config_ts, &original).unwrap();
    assert!(
        !result.status.success(),
        "expected the type check to fail once the library no longer has the declared field"
    );
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("host"),
        "expected the compiler error to name the missing field:\n{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

//! A worked `ext` for the probe tests: one library with every argument and
//! result shape a binding can declare, so each probe line is pinned.

use std::collections::BTreeMap;

use crate::ir::{
    CallArg, CallCtor, ExtLib, ExternDecl, ExternLang, ExternParam, ForeignField, ForeignLang,
    ForeignStruct, LangPath, Module, OpaqueType, Prim, ReturnsField, ReturnsLit, ReturnsValue,
    Shape, ShapeKind, SymbolCall, Tref, YieldsPos,
};

fn prim(p: Prim) -> Tref {
    Tref::Prim(p)
}

fn reference(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
}

fn param(name: &str, t: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: t,
    }
}

fn lang(lang: &str, symbol: &str, call_args: Vec<CallArg>) -> ExternLang {
    ExternLang {
        lang: lang.into(),
        symbol: symbol.into(),
        call_args,
        yields: vec![],
        returns: None,
        chain: None,
    }
}

fn block(lang: &str, name: &str, fields: &[(&str, &str)]) -> ForeignLang {
    ForeignLang {
        lang: lang.into(),
        name: Some(name.into()),
        fields: fields
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

fn decl(name: &str, params: Vec<ExternParam>, ret: Tref, langs: Vec<ExternLang>) -> ExternDecl {
    ExternDecl {
        name: name.into(),
        params,
        r#return: ret,
        langs,
        r#async: vec![],
        errors: vec![],
    }
}

/// The `gearbox` library: a `dial` handle (an interface in Go, a class in
/// TypeScript), a `dial_options` form, and one op per argument shape.
pub fn gearbox() -> ExtLib {
    let dial = reference("svc#dial");
    let options = reference("svc#dial_options");
    ExtLib {
        name: "gearbox".into(),
        langs: vec![
            LangPath {
                lang: "go".into(),
                path: "example.test/gearbox".into(),
            },
            LangPath {
                lang: "ts".into(),
                path: "@example/gearbox".into(),
            },
            LangPath {
                lang: "rust".into(),
                path: "gearbox".into(),
            },
        ],
        structs: vec![
            ForeignStruct {
                name: "dial_options".into(),
                fields: vec![
                    ForeignField {
                        name: "precision".into(),
                        r#type: prim(Prim::U8),
                    },
                    ForeignField {
                        name: "label".into(),
                        r#type: prim(Prim::String),
                    },
                ],
                langs: vec![
                    block("go", "Options", &[("precision", "int")]),
                    block("ts", "DialOptions", &[]),
                ],
            },
            ForeignStruct {
                name: "rust_only".into(),
                fields: vec![ForeignField {
                    name: "x".into(),
                    r#type: prim(Prim::String),
                }],
                langs: vec![block("rust", "RustOnly", &[])],
            },
        ],
        types: vec![OpaqueType {
            name: "dial".into(),
            langs: vec![
                block("go", "Dial[float64]", &[]),
                block("ts", "Dial<number>", &[]),
            ],
            methods: vec![
                ExternDecl {
                    r#async: vec!["ts".into()],
                    ..decl(
                        "read",
                        vec![],
                        prim(Prim::Float),
                        vec![
                            lang(
                                "go",
                                "Read",
                                vec![CallArg::Foreign("ctx context.Context".into())],
                            ),
                            lang("ts", "read", vec![]),
                        ],
                    )
                },
                decl(
                    "label",
                    vec![param("text", prim(Prim::String))],
                    prim(Prim::String),
                    vec![lang("ts", "label", vec![CallArg::Param("text".into())])],
                ),
            ],
        }],
        externs: vec![
            // A generic instantiation, a parameter under the default mapping.
            decl(
                "open",
                vec![param("value", prim(Prim::Float))],
                dial.clone(),
                vec![
                    lang("go", "Open[float64]", vec![CallArg::Param("value".into())]),
                    lang("ts", "new Dial", vec![CallArg::Param("value".into())]),
                ],
            ),
            // A nested call carrying a spelled parameter, a struct literal.
            decl(
                "tune",
                vec![
                    param("name", prim(Prim::String)),
                    param("precision", prim(Prim::U8)),
                ],
                dial.clone(),
                vec![
                    lang(
                        "go",
                        "Tune[float64]",
                        vec![
                            CallArg::Param("name".into()),
                            CallArg::SymbolCall(SymbolCall {
                                symbol: "WithPrecision".into(),
                                args: vec![CallArg::ParamAs {
                                    name: "precision".into(),
                                    spelling: "int".into(),
                                }],
                            }),
                            CallArg::Lit(serde_json::json!("fine")),
                        ],
                    ),
                    lang(
                        "ts",
                        "Dial.tune",
                        vec![
                            CallArg::Param("name".into()),
                            CallArg::Ctor(CallCtor {
                                name: "dial_options".into(),
                                fields: BTreeMap::from([
                                    ("precision".to_string(), CallArg::Param("precision".into())),
                                    ("label".to_string(), CallArg::Lit(serde_json::json!("fine"))),
                                ]),
                                spelling: None,
                            }),
                        ],
                    ),
                ],
            ),
            // A collection under its own spelling, variadic in Go.
            decl(
                "merge",
                vec![param("dials", Tref::List(Box::new(dial.clone())))],
                dial.clone(),
                vec![
                    lang(
                        "go",
                        "Merge[float64]",
                        vec![CallArg::ParamAs {
                            name: "dials".into(),
                            spelling: "...Dial[float64]".into(),
                        }],
                    ),
                    lang(
                        "ts",
                        "merge",
                        vec![CallArg::ParamAs {
                            name: "dials".into(),
                            spelling: "Dial<number>[]".into(),
                        }],
                    ),
                ],
            ),
            // `yields` naming positions to project from: a form plus a
            // count in Go (the `returns:` keeps the convention's trailing
            // error), an explicit error position in TypeScript.
            decl(
                "describe",
                vec![param("name", prim(Prim::String))],
                reference("svc#summary"),
                vec![
                    ExternLang {
                        yields: vec![
                            YieldsPos {
                                name: "opts".into(),
                                r#type: Some(options.clone()),
                                is_error: false,
                                foreign: None,
                            },
                            YieldsPos {
                                name: "n".into(),
                                r#type: Some(prim(Prim::I64)),
                                is_error: false,
                                foreign: None,
                            },
                        ],
                        returns: Some(ReturnsLit {
                            r#type: reference("svc#summary"),
                            fields: vec![
                                ReturnsField {
                                    name: "label".into(),
                                    value: ReturnsValue::Field(vec!["opts".into(), "label".into()]),
                                },
                                ReturnsField {
                                    name: "count".into(),
                                    value: ReturnsValue::Field(vec!["n".into()]),
                                },
                            ],
                        }),
                        ..lang("go", "Describe", vec![CallArg::Param("name".into())])
                    },
                    ExternLang {
                        yields: vec![
                            YieldsPos {
                                name: "err".into(),
                                r#type: None,
                                is_error: true,
                                foreign: None,
                            },
                            YieldsPos {
                                name: "raw".into(),
                                r#type: None,
                                is_error: false,
                                foreign: Some("RawSummary".into()),
                            },
                        ],
                        ..lang("ts", "describe", vec![CallArg::Param("name".into())])
                    },
                ],
            ),
            // A foreign spelling as the sole yields position in Go.
            decl(
                "raw",
                vec![],
                prim(Prim::String),
                vec![ExternLang {
                    yields: vec![
                        YieldsPos {
                            name: "e".into(),
                            r#type: None,
                            is_error: true,
                            foreign: None,
                        },
                        YieldsPos {
                            name: "r".into(),
                            r#type: None,
                            is_error: false,
                            foreign: Some("Raw".into()),
                        },
                    ],
                    ..lang("go", "Raw", vec![])
                }],
            ),
            // A class reference: TypeScript passes the class, Go has none.
            decl(
                "instantiate",
                vec![],
                dial.clone(),
                vec![
                    lang("go", "Instantiate", vec![CallArg::TypeRef("dial".into())]),
                    lang("ts", "instantiate", vec![CallArg::TypeRef("dial".into())]),
                ],
            ),
            // A return the generated SDK defines: nothing the probe can spell.
            decl(
                "summary",
                vec![],
                reference("svc#summary"),
                vec![lang("go", "Summary", vec![]), lang("ts", "summary", vec![])],
            ),
            // A parameter the probe cannot spell.
            decl(
                "stamp",
                vec![param("at", prim(Prim::Timestamp))],
                prim(Prim::Bool),
                vec![
                    lang("go", "Stamp", vec![CallArg::Param("at".into())]),
                    lang("ts", "stamp", vec![CallArg::Param("at".into())]),
                ],
            ),
            // A position TypeScript never binds.
            decl(
                "ping",
                vec![],
                prim(Prim::Bool),
                vec![lang(
                    "ts",
                    "ping",
                    vec![CallArg::Foreign("ctx context.Context".into())],
                )],
            ),
            // A struct literal of a form with no block for the language.
            decl(
                "rusty",
                vec![],
                prim(Prim::Bool),
                vec![lang(
                    "go",
                    "Rusty",
                    vec![CallArg::Ctor(CallCtor {
                        name: "rust_only".into(),
                        fields: BTreeMap::new(),
                        spelling: None,
                    })],
                )],
            ),
            // A parameter that must convert at the boundary, not just be
            // respelled: an i64 is a bigint in TypeScript, and the binding
            // says it crosses as number.
            decl(
                "reseed",
                vec![param("seed", prim(Prim::I64))],
                dial.clone(),
                vec![lang(
                    "ts",
                    "reseed",
                    vec![CallArg::ParamAs {
                        name: "seed".into(),
                        spelling: "number".into(),
                    }],
                )],
            ),
        ],
    }
}

/// A module holding `gearbox` and one generated shape (`summary`).
pub fn gearbox_module() -> Module {
    Module {
        name: "svc".into(),
        shapes: vec![Shape {
            id: "svc#summary".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits: vec![],
        }],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![gearbox()],
        tests: vec![],
    }
}

/// `module` with `gearbox` bound in `lang` alone: an ext bound in several
/// languages is gated on a declared test covering it, which a generation of
/// the types (what a probe compiles beside) runs into.
pub fn bind_only(module: &mut Module, lang: &str) {
    for lib in &mut module.ext_libs {
        lib.langs.retain(|l| l.lang == lang);
        for decl in lib
            .externs
            .iter_mut()
            .chain(lib.types.iter_mut().flat_map(|t| t.methods.iter_mut()))
        {
            decl.langs.retain(|l| l.lang == lang);
        }
    }
}

/// The module a toolchain probe test compiles: `gearbox` bound in `lang`
/// alone, cut down to the `dial` handle with its first method and the
/// `open` constructor, the two shapes a small stand-in library declares.
pub fn probe_consumer_module(lang: &str) -> Module {
    let mut m = gearbox_module();
    bind_only(&mut m, lang);
    let lib = &mut m.ext_libs[0];
    lib.structs.clear();
    lib.externs.retain(|d| d.name == "open");
    lib.types[0].methods.truncate(1);
    m
}

/// Whether no probe scratch directory is left under `root`.
pub fn scratch_free(root: &std::path::Path) -> bool {
    std::fs::read_dir(root).unwrap().all(|e| {
        !e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")
    })
}

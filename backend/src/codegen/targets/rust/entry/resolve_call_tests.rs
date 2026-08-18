//! Coverage for the extern-call leaf ([`super`]): the `returns:` projection
//! expressions, the `errors:` mapping, and `call_arg_expr`'s full variant
//! table (positional `Param` substitution included). The end-to-end "does
//! the Rust actually compile" proof lives in
//! `backend/tests/rust_ext_roundtrip.rs`.

use super::*;
use crate::codegen::targets::rust::rust_casing;
use crate::codegen::test_support::bare_entry_field;
use crate::ir::{
    CallArg, EntryCall, EntryField, ErrorBinding, ExtLib, ExternDecl, ExternParam, LangPath,
    ReturnsField, ReturnsLit, ReturnsValue, SelectArm, YieldsPos,
};

fn module_of(shapes: Vec<Shape>) -> Module {
    Module {
        name: "m".into(),
        shapes,
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
        tests: vec![],
    }
}

fn client_shape(fields: Vec<EntryField>) -> Shape {
    Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields,
            operations: vec![],
        },
        traits: vec![],
    }
}

fn shape_ref(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
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

/// A field constructed by `ns.func(args)`, with `sources` as its own
/// declared sources (`@with` for a call fallback, empty otherwise).
fn call_field(
    name: &str,
    sources: Vec<Source>,
    ns: &str,
    func: &str,
    args: Vec<CallArg>,
) -> EntryField {
    let mut field = bare_entry_field(name, Tref::Prim(Prim::String), sources);
    field.call = Some(EntryCall {
        ns: ns.into(),
        func: func.into(),
        args,
    });
    field
}

/// The bare `rust` block of an extern: an awaited call of `symbol` with
/// `call_args`, no `yields`/`returns`/`errors:`.
fn rust_lang(symbol: &str, call_args: Vec<CallArg>) -> ExternLang {
    ExternLang {
        lang: "rust".into(),
        symbol: symbol.into(),
        call_args,
        yields: vec![],
        returns: None,
        errors: vec![],
        sync: false,
        infallible: false,
        ctx: false,
    }
}

fn extern_decl(
    name: &str,
    params: Vec<ExternParam>,
    r#return: Tref,
    lang: ExternLang,
) -> ExternDecl {
    ExternDecl {
        name: name.into(),
        params,
        r#return,
        langs: vec![lang],
    }
}

/// An `ext` library named `name`, mapped to the Rust crate `path`,
/// declaring `externs` and no structs or types.
fn lib(name: &str, path: &str, externs: Vec<ExternDecl>) -> ExtLib {
    ExtLib {
        name: name.into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: path.into(),
        }],
        structs: vec![],
        types: vec![],
        externs,
    }
}

/// `companyconfig.load(region)` against a `company-config` crate whose
/// `load` takes one `region` parameter, with `sync` as given.
fn region_load_module(sync: bool) -> Module {
    let region = bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::Arg]);
    let config = call_field(
        "config",
        vec![],
        "companyconfig",
        "load",
        vec![CallArg::Ref(vec!["region".into()])],
    );
    let mut module = module_of(vec![client_shape(vec![config, region])]);
    let mut lang = rust_lang("Client::load", vec![CallArg::Ref(vec!["region".into()])]);
    lang.sync = sync;
    module.ext_libs = vec![lib(
        "companyconfig",
        "company-config",
        vec![extern_decl(
            "load",
            string_params(&["region"]),
            Tref::Prim(Prim::String),
            lang,
        )],
    )];
    module
}

/// A plain call field (no `yields`, no `returns`) against a lib that
/// declares no `errors:` mapping: `call_assign` must not panic, must
/// await the crate-qualified call, and must fall back to `Ok(v)`
/// binding straight into `dest`.
#[test]
fn a_plain_call_field_emits_an_awaited_call_with_no_projection() {
    let out = entry_text(&region_load_module(false), &rust_casing());
    assert!(out.contains("company_config::Client::load"), "{out}");
    assert!(out.contains(".await"), "{out}");
    assert!(out.contains("async fn new"), "{out}");
}

#[test]
fn a_sync_call_field_emits_without_await() {
    let out = entry_text(&region_load_module(true), &rust_casing());
    assert!(out.contains("company_config::Client::load"), "{out}");
    assert!(!out.contains(".await"), "{out}");
}

/// A `@with` field backed by a call fallback builds through
/// `ClientBuilder`, and the injected value wins over the call: this
/// exercises `with_present_cond`/`with_assign` (the plain boolean
/// `if`/`else` the shared plan wraps a call in when `@with` is also
/// declared).
#[test]
fn a_with_field_backed_by_a_call_fallback_prefers_the_injected_value() {
    let bus = call_field("bus", vec![Source::With], "companybus", "connect", vec![]);
    let mut module = module_of(vec![client_shape(vec![bus])]);
    module.ext_libs = vec![lib(
        "companybus",
        "company_bus",
        vec![extern_decl(
            "connect",
            vec![],
            Tref::Prim(Prim::String),
            rust_lang("connect", vec![]),
        )],
    )];
    let out = entry_text(&module, &rust_casing());
    assert!(out.contains(".is_some()"), "{out}");
    assert!(out.contains(".unwrap();"), "{out}");
    assert!(out.contains("company_bus::connect()"), "{out}");
}

/// A call declaring `yields`/`returns`/`errors:` projects the success
/// binding into the declared struct fields and maps the declared
/// sentinel, without panicking.
#[test]
fn a_call_field_with_yields_returns_and_errors_projects_and_maps() {
    let mut conn = call_field("conn", vec![], "companyconfig", "load", vec![]);
    conn.target = shape_ref("m#app_config");
    let app_config = Shape {
        id: "m#app_config".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    };
    let mut module = module_of(vec![client_shape(vec![conn]), app_config]);
    let mut lang = rust_lang("Client::load", vec![]);
    lang.yields = vec![
        YieldsPos {
            name: "cfg".into(),
            r#type: Some(Tref::Prim(Prim::String)),
            is_error: false,
        },
        YieldsPos {
            name: "err".into(),
            r#type: None,
            is_error: true,
        },
    ];
    lang.returns = Some(ReturnsLit {
        r#type: shape_ref("m#app_config"),
        fields: vec![ReturnsField {
            name: "endpoint".into(),
            value: ReturnsValue::Field(vec!["cfg".into(), "Host".into()]),
        }],
    });
    lang.errors = vec![ErrorBinding {
        sentinel: "ErrBusy".into(),
        r#type: "overloaded".into(),
    }];
    module.ext_libs = vec![lib(
        "companyconfig",
        "company_config",
        vec![extern_decl("load", vec![], shape_ref("m#app_config"), lang)],
    )];
    let out = entry_text(&module, &rust_casing());
    assert!(out.contains("Ok(cfg)"), "{out}");
    assert!(out.contains("endpoint: cfg.Host"), "{out}");
    assert!(out.contains("\"ErrBusy\""), "{out}");
    assert!(out.contains("ContractError"), "{out}");
}

#[test]
fn json_literal_covers_every_json_value_kind() {
    assert_eq!(json_literal(&serde_json::json!("hi")), "\"hi\".to_string()");
    assert_eq!(json_literal(&serde_json::json!(true)), "true");
    assert_eq!(json_literal(&serde_json::json!(3)), "3");
    assert_eq!(json_literal(&serde_json::json!(null)), "Default::default()");
    assert_eq!(
        json_literal(&serde_json::json!([1, 2])),
        "serde_json::json!([1,2])"
    );
}

#[test]
fn ok_pattern_destructures_a_tuple_for_more_than_one_position() {
    let a = YieldsPos {
        name: "a".into(),
        r#type: None,
        is_error: false,
    };
    let b = YieldsPos {
        name: "b".into(),
        r#type: None,
        is_error: false,
    };
    assert_eq!(ok_pattern(&[&a]), "a");
    assert_eq!(ok_pattern(&[&a, &b]), "(a, b)");
}

#[test]
fn select_expr_covers_a_field_arm_a_sources_arm_and_the_synthesized_default() {
    let select = Select {
        subject_index: None,
        subject: vec!["cfg".into(), "env".into()],
        arms: vec![
            SelectArm {
                pattern: Some(serde_json::json!("prod")),
                value: ArmValue::Field(vec!["cfg".into(), "host".into()]),
            },
            SelectArm {
                pattern: Some(serde_json::json!("dev")),
                value: ArmValue::Sources(vec![]),
            },
        ],
    };
    let out = select_expr(&select);
    assert!(out.contains("\"prod\" => cfg.host"), "{out}");
    // Every declared arm carries a pattern, so the function synthesizes
    // a trailing wildcard arm rather than leaving the match non-total.
    assert!(out.contains("_ => cfg.env.clone()"), "{out}");
}

#[test]
fn error_match_maps_every_declared_sentinel_and_falls_back_to_contract_error() {
    let out = error_match(
        "companybus",
        "send",
        &[
            ErrorBinding {
                sentinel: "ErrBusy".into(),
                r#type: "overloaded".into(),
            },
            ErrorBinding {
                sentinel: "ErrGone".into(),
                r#type: "not_found".into(),
            },
        ],
    );
    assert!(out.contains("\"ErrBusy\" =>"), "{out}");
    assert!(out.contains("\"ErrGone\" =>"), "{out}");
    assert!(out.contains("companybus.send"), "{out}");
    assert!(out.contains("ContractError"), "{out}");
}

/// The four-argument shape the every-variant test below feeds `load`
/// with: `head` in the first position (the caller passes a `Ref`, the
/// language block names the `reg` parameter), then a `List`, a `Ctor`
/// and a nested `Call`.
fn every_variant_args(head: CallArg) -> Vec<CallArg> {
    vec![
        head,
        CallArg::List(vec![CallArg::Lit(serde_json::json!(1))]),
        CallArg::Ctor(crate::ir::CallCtor {
            name: "opts".into(),
            fields: [("retries".to_string(), CallArg::Lit(serde_json::json!(3)))]
                .into_iter()
                .collect(),
        }),
        CallArg::Call(Box::new(EntryCall {
            ns: "companyauth".into(),
            func: "sign".into(),
            args: vec![],
        })),
    ]
}

/// A call whose `call_args` mix every `CallArg` variant (`Param`,
/// `List`, `Ctor`, a nested `Call`), and whose `yields` carries two
/// non-error positions, exercises `call_arg_expr`'s full match and the
/// `ok_pattern` tuple branch together. The `Param` names the extern's
/// own `reg` parameter, which the caller feeds from the `region` field:
/// the emitted argument must be the field the caller passed, not a
/// same-named `reg` read.
#[test]
fn a_call_field_with_every_call_arg_variant_and_a_nested_call_emits_without_panicking() {
    let region = bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::Arg]);
    let token = call_field("token", vec![], "companyauth", "sign", vec![]);
    let config = call_field(
        "config",
        vec![],
        "companyconfig",
        "load",
        every_variant_args(CallArg::Ref(vec!["region".into()])),
    );
    let mut module = module_of(vec![client_shape(vec![token, config, region])]);
    let mut load = rust_lang(
        "Client::load",
        every_variant_args(CallArg::Param("reg".into())),
    );
    load.yields = ["a", "b"]
        .iter()
        .map(|n| YieldsPos {
            name: (*n).to_string(),
            r#type: Some(Tref::Prim(Prim::String)),
            is_error: false,
        })
        .collect();
    let mut config_lib = lib(
        "companyconfig",
        "company_config",
        vec![extern_decl(
            "load",
            string_params(&["reg", "ids", "opts", "sig"]),
            Tref::Prim(Prim::String),
            load,
        )],
    );
    config_lib.structs = vec![crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![crate::ir::ForeignField {
            name: "retries".into(),
            r#type: Tref::Prim(Prim::I32),
        }],
    }];
    module.ext_libs = vec![
        lib(
            "companyauth",
            "company-auth",
            vec![extern_decl(
                "sign",
                vec![],
                Tref::Prim(Prim::String),
                rust_lang("sign", vec![]),
            )],
        ),
        config_lib,
    ];
    let out = entry_text(&module, &rust_casing());
    assert!(out.contains("(s.region).clone()"), "{out}");
    assert!(!out.contains("s.reg)"), "{out}");
    assert!(out.contains("vec![1]"), "{out}");
    assert!(out.contains("opts { retries: 3 }"), "{out}");
    assert!(out.contains("company_auth::sign().await"), "{out}");
    assert!(out.contains("Ok((a, b))"), "{out}");
}

/// A `Param` naming a parameter the caller passed no argument for is a
/// frontend arity gap, and the emitter refuses it loudly rather than
/// guessing at a same-named field.
#[test]
#[should_panic(expected = "extern param \"reg\" has no argument at its position")]
fn a_param_with_no_argument_at_its_position_panics() {
    let config = call_field("config", vec![], "companyconfig", "load", vec![]);
    let mut module = module_of(vec![client_shape(vec![config])]);
    module.ext_libs = vec![lib(
        "companyconfig",
        "company_config",
        vec![extern_decl(
            "load",
            string_params(&["reg"]),
            Tref::Prim(Prim::String),
            rust_lang("load", vec![CallArg::Param("reg".into())]),
        )],
    )];
    let _ = entry_text(&module, &rust_casing());
}

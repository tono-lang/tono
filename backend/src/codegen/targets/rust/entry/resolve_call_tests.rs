//! Coverage for the extern-call leaf ([`super`]): the `returns:` projection
//! expressions, the `errors:` mapping, and `call_arg_expr`'s full variant
//! table (positional `Param` substitution included). The end-to-end "does
//! the Rust actually compile" proof lives in
//! `backend/tests/rust_ext_roundtrip.rs`.

use super::*;
use crate::codegen::targets::rust::rust_casing;
use crate::codegen::test_support::bare_entry_field;
use crate::ir::{
    CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternParam, LangPath, ReturnsField,
    ReturnsLit, ReturnsValue, YieldsPos,
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
        chain: None,
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
        r#async: vec![],
        errors: vec![],
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
    let lang = rust_lang(
        "company_config::Client::load",
        vec![CallArg::Ref(vec!["region".into()])],
    );
    let mut decl = extern_decl(
        "load",
        string_params(&["region"]),
        Tref::Prim(Prim::String),
        lang,
    );
    decl.r#async = if sync { vec![] } else { vec!["rust".into()] };
    module.ext_libs = vec![lib("companyconfig", "company-config", vec![decl])];
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
    let mut lang = rust_lang("company_config::Client::load", vec![]);
    lang.yields = vec![
        YieldsPos {
            name: "cfg".into(),
            r#type: Some(Tref::Prim(Prim::String)),
            is_error: false,
            foreign: None,
        },
        YieldsPos {
            name: "err".into(),
            r#type: None,
            is_error: true,
            foreign: None,
        },
    ];
    lang.returns = Some(ReturnsLit {
        r#type: shape_ref("m#app_config"),
        fields: vec![ReturnsField {
            name: "endpoint".into(),
            value: ReturnsValue::Field(vec!["cfg".into(), "Host".into()]),
        }],
    });
    module.shapes.push(Shape {
        id: "m#overloaded".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![crate::ir::Trait {
            id: "foreign".into(),
            value: serde_json::json!([
                {"lang": "rust", "name": "Error::Busy", "fields": {"message": "to_string()"}}
            ]),
        }],
    });
    let mut decl = extern_decl("load", vec![], shape_ref("m#app_config"), lang);
    decl.errors = vec!["m#overloaded".into()];
    module.ext_libs = vec![lib("companyconfig", "company_config", vec![decl])];
    let out = entry_text(&module, &rust_casing());
    assert!(out.contains("Ok(cfg)"), "{out}");
    assert!(out.contains("endpoint: cfg.Host"), "{out}");
    assert!(
        out.contains("matches!(e, company_config::Error::Busy { .. })"),
        "{out}"
    );
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
fn error_match_recognizes_every_declared_error_and_falls_back_to_contract_error() {
    let error_shape = |id: &str, name: &str| Shape {
        id: id.to_string(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![crate::ir::Trait {
            id: "foreign".into(),
            value: serde_json::json!([
                {"lang": "rust", "name": name, "fields": {"message": "to_string()"}}
            ]),
        }],
    };
    let module = module_of(vec![
        error_shape("m#overloaded", "Error::Busy"),
        error_shape("m#not_found", "Error::Gone"),
    ]);
    let lib = crate::ir::ExtLib {
        name: "companybus".into(),
        langs: vec![crate::ir::LangPath {
            lang: "rust".into(),
            path: "companybus".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![],
    };
    let out = error_match(
        &module,
        &lib,
        "companybus",
        "send",
        &["m#overloaded".to_string(), "m#not_found".to_string()],
    );
    assert!(
        out.contains("matches!(e, companybus::Error::Busy { .. })"),
        "{out}"
    );
    assert!(
        out.contains("matches!(e, companybus::Error::Gone { .. })"),
        "{out}"
    );
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
            spelling: None,
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
            foreign: None,
        })
        .collect();
    // The positions are read by a projection, so the call keeps the
    // convention's `Result` and the tuple destructures out of `Ok`.
    load.returns = Some(ReturnsLit {
        r#type: shape_ref("m#app_config"),
        fields: vec![ReturnsField {
            name: "endpoint".into(),
            value: ReturnsValue::Field(vec!["a".into()]),
        }],
    });
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
        langs: vec![crate::ir::ForeignLang {
            lang: "rust".into(),
            name: Some("Opts".into()),
            fields: Default::default(),
        }],
    }];
    let mut sign = extern_decl(
        "sign",
        vec![],
        Tref::Prim(Prim::String),
        rust_lang("sign", vec![]),
    );
    sign.r#async = vec!["rust".into()];
    module.ext_libs = vec![lib("companyauth", "company-auth", vec![sign]), config_lib];
    let out = entry_text(&module, &rust_casing());
    assert!(out.contains("(s.region).clone()"), "{out}");
    assert!(!out.contains("s.reg)"), "{out}");
    assert!(out.contains("vec![1]"), "{out}");
    assert!(out.contains("company_config::Opts { retries: 3 }"), "{out}");
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

/// The conversions Rust knows for a parameter spelled under its own type,
/// and the refusal naming both types when there is none.
#[test]
fn coerce_knows_some_borrow_and_identity_and_refuses_the_rest() {
    let module = module_of(vec![]);
    let lib = lib("companyconfig", "company-config", vec![]);
    let string = Tref::Prim(Prim::String);
    assert_eq!(coerce(&module, &lib, &string, "String", "v").unwrap(), "v");
    assert_eq!(
        coerce(&module, &lib, &string, "Option<String>", "v").unwrap(),
        "Some(v)"
    );
    assert_eq!(coerce(&module, &lib, &string, "&str", "v").unwrap(), "&v");
    assert_eq!(
        coerce(&module, &lib, &string, "&String", "v").unwrap(),
        "&v"
    );
    let list = Tref::List(Box::new(Tref::Prim(Prim::Float)));
    assert_eq!(coerce(&module, &lib, &list, "Vec<f64>", "v").unwrap(), "v");
    let err = coerce(&module, &lib, &string, "u8", "v").unwrap_err();
    assert!(err.contains("no conversion from String to u8"), "{err}");
}

/// A literal naming none of the lib's forms builds one of the module's own
/// structs: the generated type's struct literal, its fields named as the
/// types module names them (`@rename(rust)` honored), not crate-qualified,
/// and an optional member the literal leaves out written `None`, since
/// Rust has no zero value to fall back on.
#[test]
fn ctor_expr_builds_a_generated_struct_as_its_own_literal() {
    use crate::codegen::test_support::{member, member_with, structure, trait_of};
    let module = module_of(vec![structure(
        "m#reading",
        vec![
            member("value", Tref::Prim(Prim::Float), true),
            member_with(
                "sample_rate",
                Tref::Prim(Prim::U32),
                true,
                vec![trait_of("core#rename", serde_json::json!({ "rust": "hz" }))],
            ),
            member("note", Tref::Prim(Prim::String), false),
        ],
    )]);
    let lib = lib("companyconfig", "company-config", vec![]);
    let ctor = CallCtor {
        name: "reading".into(),
        fields: [
            ("sample_rate".to_string(), CallArg::Ref(vec!["rate".into()])),
            ("value".to_string(), CallArg::Ref(vec!["v".into()])),
        ]
        .into_iter()
        .collect(),
        spelling: None,
    };
    let out = ctor_expr(
        &module,
        &lib,
        "company_config",
        &ctor,
        &["(rate).clone()".to_string(), "(v).clone()".to_string()],
    );
    assert_eq!(
        out,
        "Reading { hz: (rate).clone(), value: (v).clone(), note: None }"
    );
}

/// A form's literal names the library's own type from its `rust` block and
/// converts a field the block spells under another type.
#[test]
fn ctor_expr_builds_a_form_from_its_rust_block() {
    let module = module_of(vec![]);
    let mut lib = lib("companyconfig", "company-config", vec![]);
    let mut spelled = std::collections::BTreeMap::new();
    spelled.insert("n".to_string(), "Option<String>".to_string());
    lib.structs = vec![crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![crate::ir::ForeignField {
            name: "n".into(),
            r#type: Tref::Prim(Prim::String),
        }],
        langs: vec![crate::ir::ForeignLang {
            lang: "rust".into(),
            name: Some("Opts".into()),
            fields: spelled,
        }],
    }];
    let ctor = CallCtor {
        name: "opts".into(),
        fields: [("n".to_string(), CallArg::Lit(serde_json::json!("x")))]
            .into_iter()
            .collect(),
        spelling: None,
    };
    let out = ctor_expr(
        &module,
        &lib,
        "company_config",
        &ctor,
        &["\"x\".to_string()".to_string()],
    );
    assert_eq!(out, "company_config::Opts { n: Some(\"x\".to_string()) }");
}

/// The literal under a spelling of its own goes through the same conversion
/// a spelled parameter does: `&Opts` lends it for the call, `Option<Opts>`
/// wraps it, the form's own type passes it as is, and a spelling with no
/// conversion is refused naming both types.
#[test]
fn a_spelled_form_literal_is_lent_or_wrapped() {
    let module = module_of(vec![]);
    let mut lib = lib("companyconfig", "company-config", vec![]);
    lib.structs = vec![crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![],
        langs: vec![crate::ir::ForeignLang {
            lang: "rust".into(),
            name: Some("Opts".into()),
            fields: Default::default(),
        }],
    }];
    let literal = |spelling: &str| CallCtor {
        name: "opts".into(),
        fields: Default::default(),
        spelling: Some(spelling.into()),
    };
    let render =
        |spelling: &str| ctor_expr(&module, &lib, "company_config", &literal(spelling), &[]);
    assert_eq!(render("&Opts"), "&company_config::Opts {  }");
    assert_eq!(render("Option<Opts>"), "Some(company_config::Opts {  })");
    assert_eq!(render("Opts"), "company_config::Opts {  }");
    let coerces = |spelling: &str| {
        crate::codegen::targets::rust::entry::form_spelling_coerces(
            &module,
            &lib,
            &lib.structs[0],
            spelling,
        )
    };
    assert!(coerces("&Opts").is_ok());
    let err = coerces("u8").unwrap_err();
    assert!(
        err.contains("no conversion from company_config::Opts to u8"),
        "{err}"
    );
    let mut blockless = lib.structs[0].clone();
    blockless.langs.clear();
    assert!(crate::codegen::targets::rust::entry::form_spelling_coerces(
        &module, &lib, &blockless, "u8"
    )
    .is_ok());
}

/// A spelled parameter and a nested foreign call inside a field's own
/// construction call: the parameter is lent as `&str`, the nested symbol is
/// crate-qualified.
#[test]
fn a_call_field_lends_a_spelled_parameter_and_qualifies_a_nested_call() {
    let region = bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::Arg]);
    let config = call_field(
        "config",
        vec![],
        "companyconfig",
        "load",
        vec![CallArg::Ref(vec!["region".into()])],
    );
    let mut module = module_of(vec![client_shape(vec![config, region])]);
    let lang = rust_lang(
        "load",
        vec![
            CallArg::ParamAs {
                name: "region".into(),
                spelling: "&str".into(),
            },
            CallArg::SymbolCall(crate::ir::SymbolCall {
                symbol: "Retries".into(),
                args: vec![CallArg::Lit(serde_json::json!(3))],
            }),
        ],
    );
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
    let out = entry_text(&module, &rust_casing());
    // The `&str` coercion happens inside the resolver, over its own
    // `region` parameter (not a `s.`-prefixed settings read: that read is
    // the call site's job, passing the already-resolved value in).
    assert!(out.contains("&(region).clone()"), "{out}");
    assert!(out.contains("company_config::Retries(3)"), "{out}");
}

/// A `yields` list that is the call's signature and places no `error` is a
/// plain value: the resolver binds it through a plain `let`, with no
/// `match` to run — but the resolver's own signature is always
/// `Result<T, TonoError>` (every resolver is called through `?`), so an
/// infallible call still returns through `Ok(..)`.
#[test]
fn a_signature_yields_list_without_an_error_binds_the_plain_value() {
    let mut module = region_load_module(true);
    module.ext_libs[0].externs[0].langs[0].yields = vec![YieldsPos {
        name: "cfg".into(),
        r#type: Some(Tref::Prim(Prim::String)),
        is_error: false,
        foreign: None,
    }];
    let out = entry_text(&module, &rust_casing());
    assert!(
        out.contains("let cfg = company_config::Client::load((region).clone());"),
        "{out}"
    );
    assert!(out.contains("Ok(cfg)"), "{out}");
    assert!(!out.contains("match "), "{out}");
}

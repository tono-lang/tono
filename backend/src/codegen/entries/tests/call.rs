//! The extern-call field-source tests: DAG ordering across a call
//! chain, cycle detection over call-arg refs, `is_guaranteed` classification
//! for plain and `@with`-fallback call fields, and `FieldShape::Call`
//! dispatch regardless of the target's own shape. Fixtures (`field`,
//! `entry_shape`, `module_of`) come from the parent module via `super::*`.

use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::{CallArg, EntryCall, ExtLib, ExternDecl, ExternLang, OpaqueType};

pub(super) fn call_field(name: &str, ns: &str, func: &str, args: Vec<CallArg>) -> EntryField {
    let mut f = field(name, vec![]);
    f.call = Some(EntryCall {
        ns: ns.into(),
        func: func.into(),
        args,
    });
    f
}

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

#[test]
fn a_three_level_call_chain_orders_with_no_declared_order() {
    // config reads .service/.region; auth and bus each read .config. Declared
    // out of dependency order, on purpose, so the DAG (not declaration order)
    // has to produce the chain.
    let auth = call_field("auth", "companyauth", "sign", vec![call_ref(&["config"])]);
    let bus = call_field(
        "bus",
        "companybus",
        "connect",
        vec![
            call_ref(&["config", "endpoint"]),
            call_ref(&["config", "token"]),
        ],
    );
    let config = call_field(
        "config",
        "companyconfig",
        "load",
        vec![call_ref(&["service"]), call_ref(&["region"])],
    );
    let service = field("service", vec![Source::Default(json!("notes"))]);
    let region = field("region", vec![Source::Arg]);
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![auth, bus, config, service, region],
    )]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
    assert!(pos("service") < pos("config"));
    assert!(pos("region") < pos("config"));
    assert!(pos("config") < pos("auth"));
    assert!(pos("config") < pos("bus"));
}

#[test]
fn a_cycle_between_call_sourced_fields_appends_in_declaration_order_instead_of_dropping() {
    let a = call_field("a", "ns", "f", vec![call_ref(&["b"])]);
    let b = call_field("b", "ns", "g", vec![call_ref(&["a"])]);
    let module = module_of(vec![entry_shape("m#client", vec![a, b])]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["a", "b"]);
}

#[test]
fn a_plain_call_field_is_guaranteed_when_its_args_are() {
    let region = field("region", vec![Source::Arg]);
    let config = call_field("config", "ns", "load", vec![call_ref(&["region"])]);
    let module = module_of(vec![entry_shape("m#client", vec![config, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = entry.fields.iter().find(|f| f.name == "config").unwrap();
    assert!(entry.is_guaranteed(config));
}

#[test]
fn a_plain_call_field_is_not_guaranteed_when_it_reads_a_non_guaranteed_sibling() {
    let region = field("region", vec![Source::Env(EnvName::Name("REGION".into()))]);
    let config = call_field("config", "ns", "load", vec![call_ref(&["region"])]);
    let module = module_of(vec![entry_shape("m#client", vec![config, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = entry.fields.iter().find(|f| f.name == "config").unwrap();
    assert!(!entry.is_guaranteed(config));
}

#[test]
fn a_with_field_backed_by_a_guaranteed_call_fallback_is_guaranteed() {
    // An injectable handle with construction as fallback is
    // guaranteed the same way a plain call is (either the caller injects it,
    // or the fallback call runs and its own reads are guaranteed) — `@with`
    // does not relax the rule, both reduce to the same check.
    let region = field("region", vec![Source::Arg]);
    let mut bus = call_field("bus", "companybus", "connect", vec![call_ref(&["region"])]);
    bus.sources = vec![Source::With];
    let module = module_of(vec![entry_shape("m#client", vec![bus, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let bus = entry.fields.iter().find(|f| f.name == "bus").unwrap();
    assert!(entry.is_guaranteed(bus));
}

#[test]
fn a_with_field_backed_by_a_non_guaranteed_call_fallback_is_not_guaranteed() {
    let region = field("region", vec![Source::Env(EnvName::Name("REGION".into()))]);
    let mut bus = call_field("bus", "companybus", "connect", vec![call_ref(&["region"])]);
    bus.sources = vec![Source::With];
    let module = module_of(vec![entry_shape("m#client", vec![bus, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let bus = entry.fields.iter().find(|f| f.name == "bus").unwrap();
    assert!(!entry.is_guaranteed(bus));
}

#[test]
fn a_call_sourced_field_reports_the_call_shape_regardless_of_its_target() {
    let plain = call_field("config", "ns", "load", vec![]);
    let mut opaque = call_field("bus", "ns", "connect", vec![]);
    // An opaque handle's type has no entry in `module.shapes` at all.
    opaque.target = Tref::Ref {
        id: "ext#publisher".into(),
        args: vec![],
    };
    let module = module_of(vec![entry_shape("m#client", vec![plain, opaque])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    for name in ["config", "bus"] {
        let f = entry.fields.iter().find(|f| f.name == name).unwrap();
        assert!(matches!(entry.field_shape(f, &module), FieldShape::Call));
    }
}

pub(super) fn model_of(module: Module) -> crate::ir::Model {
    crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    }
}

/// An `ext` block declaring one extern bound for `langs`. A call field is only
/// well-formed when its `ns.fn` resolves to a declaration like this carrying a
/// binding for the target being generated, so fixtures that expect to clear
/// validation have to supply it.
pub(super) fn ext_lib_with_extern(lib: &str, name: &str, langs: &[&str]) -> ExtLib {
    ExtLib {
        name: lib.into(),
        langs: vec![],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: name.into(),
            params: vec![],
            r#return: Tref::Prim(crate::ir::Prim::String),
            langs: langs
                .iter()
                .map(|l| ExternLang {
                    lang: (*l).into(),
                    symbol: "Load".into(),
                    call_args: vec![],
                    yields: vec![],
                    returns: None,
                    chain: None,
                })
                .collect(),
            r#async: vec![],
            errors: vec![],
        }],
    }
}

/// A run generating for several targets is only as capable as its weakest
/// one: Go emits the construct, Rust does not, and a green Go pass says
/// nothing about the Rust files written by the same `tono gen`. The gate reads
/// every requested target rather than any one of them.
#[test]
fn a_mixed_target_run_rejects_even_though_one_target_supports_ext() {
    let model = model_with_a_call_sourced_field();
    assert!(super::validate_entries(&model, &[TargetKind::Go]).is_ok());
    let err = super::validate_entries(&model, &[TargetKind::Go, TargetKind::Rust]).unwrap_err();
    assert!(err.contains("rust"), "names the target that cannot: {err}");
}

/// A run naming no target vouches for nothing; treating an empty list as
/// "every target supports it" would let the construct through unguarded.
#[test]
fn an_empty_target_list_does_not_vacuously_permit_ext() {
    let model = model_with_a_call_sourced_field();
    assert!(super::validate_entries(&model, &[]).is_err());
}

/// The call has to name a declared extern carrying a binding for the target
/// about to emit it. Without this the emitter meets the gap as a pipeline
/// defect (a panic) instead of an authoring error with a message.
#[test]
fn an_unresolvable_call_is_diagnosed_rather_than_reaching_the_emitter() {
    // No ext block at all.
    let bare = model_of(module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]));
    let err = super::validate_entries(&bare, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("no ext block named ns"), "{err}");

    // Declared, but the extern the call names is missing.
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "missing", vec![])],
    )]);
    module.ext_libs = vec![ext_lib_with_extern("ns", "load", &["go"])];
    let err = super::validate_entries(&model_of(module), &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("declares no extern named missing"), "{err}");

    // Declared, but with no binding for the target being generated.
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    module.ext_libs = vec![ext_lib_with_extern("ns", "load", &["ts"])];
    let err = super::validate_entries(&model_of(module), &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("declares no go block"), "{err}");
}

/// A `call:` line whose own argument tree uses another declared extern's
/// call (a cross-extern `ns.fn(...)`, not the bare-symbol nested form) is
/// rejected for a target that cannot render it yet, naming that target;
/// Rust, which already can, still passes. Without this gate the construct
/// would reach Go/TypeScript codegen and either write wrong output (`nil`)
/// or panic mid-generation instead of failing as a clean authoring error.
#[test]
fn a_cross_extern_call_as_a_call_argument_is_rejected_for_targets_that_cannot_render_it() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    let nested = CallArg::Call(Box::new(EntryCall {
        ns: "ns".into(),
        func: "load".into(),
        args: vec![],
    }));
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![nested.clone()];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("go codegen cannot render that yet"), "{err}");

    let err = super::validate_entries(&model, &[TargetKind::TypeScript]).unwrap_err();
    assert!(
        err.contains("typescript codegen cannot render that yet"),
        "{err}"
    );

    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
}

fn model_with_a_call_sourced_field() -> crate::ir::Model {
    let config = call_field("config", "ns", "load", vec![]);
    let mut module = module_of(vec![entry_shape("m#client", vec![config])]);
    module.ext_libs = vec![ext_lib_with_extern("ns", "load", &["go"])];
    model_of(module)
}

/// A call-sourced field with no binding for the target being generated is
/// rejected instead of reaching the emitter as a pipeline defect (a panic).
/// This model's extern declares only a `go` block, so every other target
/// meets it through `call_resolves`'s per-target lang-binding check, not
/// the coarser "target cannot emit calls at all" gate (every target emits
/// calls now).
#[test]
fn gen_rejects_a_call_sourced_field_with_no_binding_for_the_target() {
    let model = model_with_a_call_sourced_field();
    let err = super::validate_entries(&model, &[TargetKind::Rust]).unwrap_err();
    assert!(err.contains("config"), "{err}");
    assert!(err.contains("declares no rust block"), "{err}");
    assert!(super::validate_entries(&model, &[TargetKind::Go]).is_ok());
}

/// A call-sourced field whose extern declares a block for every requested
/// target validates cleanly, including a config member's own call source.
#[test]
fn gen_accepts_a_call_sourced_field_and_config_member_once_every_target_has_a_binding() {
    let member = call_field("token", "ns", "load", vec![]);
    let config_shape = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config {
            fields: vec![member],
        },
        traits: vec![],
    };
    let mut conf = field("conf", vec![]);
    conf.target = Tref::Ref {
        id: "m#conf".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![conf]), config_shape]);
    module.ext_libs = vec![ext_lib_with_extern("ns", "load", &["rust", "go"])];
    let model = model_of(module);
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
    assert!(super::validate_entries(&model, &[TargetKind::Go]).is_ok());
}

fn ext_lib_with_handle(lib: &str, handle: &str) -> ExtLib {
    ExtLib {
        name: lib.into(),
        langs: vec![],
        structs: vec![],
        types: vec![OpaqueType {
            name: handle.into(),
            langs: ["go", "ts", "rust"]
                .into_iter()
                .map(|l| crate::ir::ForeignLang {
                    lang: l.into(),
                    name: Some(if l == "go" {
                        "*Handle".into()
                    } else {
                        "Handle".into()
                    }),
                    fields: Default::default(),
                })
                .collect(),
            methods: vec![],
        }],
        externs: vec![],
    }
}

/// The same handle-declaring lib as [`ext_lib_with_handle`], plus a free
/// constructor extern (bound for every target this codegen now emits
/// handles for) so a `@with`/`call` field targeting the handle actually
/// resolves end to end instead of only reaching the scoping check.
fn ext_lib_with_handle_ctor(lib: &str, handle: &str, ctor: &str) -> ExtLib {
    let mut l = ext_lib_with_handle(lib, handle);
    l.externs.push(ExternDecl {
        name: ctor.into(),
        params: vec![],
        r#return: Tref::Ref {
            id: format!("{lib}#{handle}"),
            args: vec![],
        },
        langs: vec!["rust", "go", "typescript"]
            .into_iter()
            .map(|l| ExternLang {
                lang: l.into(),
                symbol: "Connect".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                chain: None,
            })
            .collect(),
        r#async: vec![],
        errors: vec![],
    });
    l
}

/// An injectable-handle field (`bus: c.h @with = c.conn(...)`)
/// targets an opaque type declared in the module's own `ext` block. That id
/// lives in `ext_libs`, not `module.shapes`; the module-scoping check must
/// recognize it as same-module instead of misreporting it as an out-of-module
/// reference. Every target this codegen emits handles for resolves the field
/// cleanly once the constructor is bound for it.
#[test]
fn a_with_and_call_field_targeting_an_ext_block_handle_is_same_module() {
    let mut bus = call_field("bus", "c", "conn", vec![]);
    bus.sources = vec![Source::With];
    bus.target = Tref::Ref {
        id: "c#h".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![bus])]);
    module.ext_libs = vec![ext_lib_with_handle_ctor("c", "h", "conn")];
    let model = model_of(module);
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
}

/// A plain `@arg`-injected handle field (`bus: c.h @arg`, no call at all)
/// carries no `field.call`. Every target this codegen emits handles for now
/// spells the foreign target type, so this reaches codegen cleanly; only
/// forwarding such a field into another extern call is still rejected
/// (`an_injected_handle_forwarded_into_another_call_is_named_and_refused`).
#[test]
fn gen_accepts_a_plain_arg_injected_foreign_handle_field() {
    let mut bus = field("bus", vec![Source::Arg]);
    bus.target = Tref::Ref {
        id: "c#h".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![bus])]);
    module.ext_libs = vec![ext_lib_with_handle("c", "h")];
    let model = model_of(module);
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
}

/// A parameter spelled under its own foreign type must be one the target
/// can coerce into: `&str` is a Rust conversion (a `String` is lent), Go
/// has none for it, and TypeScript passes it structurally (`&str` is not a
/// TypeScript primitive, so `tsc` grades it). The refusal names the site,
/// the parameter and both types.
#[test]
fn a_parameter_spelling_is_checked_against_what_the_target_can_coerce() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field(
            "config",
            "ns",
            "load",
            vec![CallArg::Lit(serde_json::json!("x"))],
        )],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    lib.langs = ["go", "ts", "rust"]
        .into_iter()
        .map(|l| crate::ir::LangPath {
            lang: l.into(),
            path: "ns-lib".into(),
        })
        .collect();
    lib.externs[0].params = vec![crate::ir::ExternParam {
        name: "region".into(),
        r#type: Tref::Prim(crate::ir::Prim::String),
    }];
    for lang in lib.externs[0].langs.iter_mut() {
        // Nested inside a nested call, so the walker is exercised too.
        lang.call_args = vec![CallArg::SymbolCall(crate::ir::SymbolCall {
            symbol: "Wrap".into(),
            args: vec![CallArg::ParamAs {
                name: "region".into(),
                spelling: "&str".into(),
            }],
        })];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("config = ns.load(..)"), "{err}");
    assert!(err.contains("passes region as #(&str)"), "{err}");
    assert!(err.contains("no conversion"), "{err}");
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());
}

/// TypeScript converts across the number/bigint divide (an i64 spelled
/// `number`) and refuses any other primitive spelling, naming both types
/// the way Go and Rust already do.
#[test]
fn a_typescript_spelling_coerces_across_bigint_and_refuses_other_primitives() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field(
            "config",
            "ns",
            "load",
            vec![CallArg::Lit(serde_json::json!(1))],
        )],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["ts"]);
    lib.langs = vec![crate::ir::LangPath {
        lang: "ts".into(),
        path: "ns-lib".into(),
    }];
    lib.externs[0].params = vec![crate::ir::ExternParam {
        name: "port".into(),
        r#type: Tref::Prim(crate::ir::Prim::I64),
    }];
    lib.externs[0].langs[0].call_args = vec![CallArg::ParamAs {
        name: "port".into(),
        spelling: "number".into(),
    }];
    module.ext_libs = vec![lib];
    let model = model_of(module.clone());
    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());

    module.ext_libs[0].externs[0].params[0].r#type = Tref::Prim(crate::ir::Prim::String);
    let model = model_of(module);
    let err = super::validate_entries(&model, &[TargetKind::TypeScript]).unwrap_err();
    assert!(err.contains("passes port as #(number)"), "{err}");
    assert!(err.contains("no conversion from string to number"), "{err}");
}

/// A struct literal under a spelling of its own (`opts { .. }: #(&Options)`)
/// must be coercible into that spelling for every target the binding
/// covers: each target names both types when it has no conversion, and the
/// site is named. The form's own block keeps the plain type either way.
#[test]
fn a_spelled_form_literal_must_coerce_for_each_target() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "rust", "ts"]);
    lib.langs = ["go", "rust", "ts"]
        .into_iter()
        .map(|l| crate::ir::LangPath {
            lang: l.into(),
            path: "ns-lib".into(),
        })
        .collect();
    lib.structs = vec![crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![],
        langs: ["go", "rust", "ts"]
            .into_iter()
            .map(|l| crate::ir::ForeignLang {
                lang: l.into(),
                name: Some("Options".into()),
                fields: Default::default(),
            })
            .collect(),
    }];
    let spelled = |spelling: &str| {
        vec![CallArg::Ctor(crate::ir::CallCtor {
            name: "opts".into(),
            fields: Default::default(),
            spelling: Some(spelling.into()),
        })]
    };
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = spelled("&Options");
    }
    module.ext_libs = vec![lib];
    let model = model_of(module.clone());
    let all = [TargetKind::Go, TargetKind::Rust, TargetKind::TypeScript];
    assert!(super::validate_entries(&model, &all).is_ok());

    for lang in module.ext_libs[0].externs[0].langs.iter_mut() {
        lang.call_args = spelled("string");
    }
    let model = model_of(module);
    for (target, from) in [
        (TargetKind::Go, "ns.Options"),
        (TargetKind::Rust, "ns_lib::Options"),
        (TargetKind::TypeScript, "Options"),
    ] {
        let err = super::validate_entries(&model, &[target]).unwrap_err();
        assert!(
            err.contains("passes the opts literal as #(string)"),
            "{err}"
        );
        assert!(
            err.contains(&format!("no conversion from {from} to string")),
            "{err}"
        );
    }
}

/// A struct literal in a binding names a form that must exist for the
/// target (a block for its language), and a field the block spells must be
/// coercible from the form's declared type.
#[test]
fn a_foreign_form_must_declare_a_block_for_the_target_it_is_built_in() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "rust"]);
    lib.langs = ["go", "rust"]
        .into_iter()
        .map(|l| crate::ir::LangPath {
            lang: l.into(),
            path: "ns-lib".into(),
        })
        .collect();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("n".to_string(), "Option<u8>".to_string());
    lib.structs = vec![crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![crate::ir::ForeignField {
            name: "n".into(),
            r#type: Tref::Prim(crate::ir::Prim::String),
        }],
        langs: vec![crate::ir::ForeignLang {
            lang: "rust".into(),
            name: Some("Opts".into()),
            fields,
        }],
    }];
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::List(vec![CallArg::Ctor(crate::ir::CallCtor {
            name: "opts".into(),
            fields: Default::default(),
            spelling: None,
        })])];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("struct opts declares no go block"), "{err}");
    let err = super::validate_entries(&model, &[TargetKind::Rust]).unwrap_err();
    assert!(err.contains("spells n as #(Option<u8>)"), "{err}");
}

/// A handle field needs a storage type for every target it is emitted in:
/// nothing is derived from the handle's name.
#[test]
fn a_handle_with_no_block_for_the_target_has_no_storage_and_is_refused() {
    let mut bus = call_field("bus", "c", "connect", vec![]);
    bus.target = Tref::Ref {
        id: "c#h".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![bus])]);
    let mut lib = ext_lib_with_handle_ctor("c", "h", "connect");
    lib.types[0].langs.retain(|l| l.lang != "go");
    module.ext_libs = vec![lib];
    let model = model_of(module);
    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("handle c.h declares no go block"), "{err}");
    assert!(err.contains("no storage type"), "{err}");
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
}

/// A reference inside a spelling (`Memo[.reading]`) names one of the
/// module's own types; one no shape answers is refused before any emitter
/// runs, naming the site and the reference. A word of a spelling is never
/// matched against the module's types, so a library name colliding with a
/// generated one is not a diagnostic.
#[test]
fn a_spelling_reference_must_name_a_declared_type() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go"]);
    lib.langs = vec![crate::ir::LangPath {
        lang: "go".into(),
        path: "ns-lib".into(),
    }];
    lib.externs[0].langs[0].symbol = "Remember[.nothing]".into();
    module.ext_libs = vec![lib];
    let err = super::validate_entries(&model_of(module.clone()), &[TargetKind::Go]).unwrap_err();
    assert!(
        err.contains(
            "ns.load: the go block spells #(Remember[.nothing]), which references .nothing"
        ) && err.contains("declares no type named nothing"),
        "{err}"
    );

    module.ext_libs[0].externs[0].langs[0].symbol = "Remember[.client]".into();
    let resolved = super::validate_entries(&model_of(module.clone()), &[TargetKind::Go]);
    assert!(
        !resolved.as_ref().is_err_and(|e| e.contains("references")),
        "{resolved:?}"
    );

    // The collision itself: the library's own `Client`, bare, in a module
    // generating a `Client` of its own, is the library's word and passes.
    module.ext_libs[0].externs[0].langs[0].symbol = "*Client".into();
    let collision = super::validate_entries(&model_of(module), &[TargetKind::Go]);
    assert!(
        !collision.as_ref().is_err_and(|e| e.contains("references")),
        "{collision:?}"
    );
}

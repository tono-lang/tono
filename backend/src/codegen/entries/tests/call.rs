//! The extern-call field-source tests: DAG ordering across a call
//! chain, cycle detection over call-arg refs, `is_guaranteed` classification
//! for plain and `@with`-fallback call fields, and `FieldShape::Call`
//! dispatch regardless of the target's own shape. Fixtures (`field`,
//! `entry_shape`, `module_of`) come from the parent module via `super::*`.

use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::{CallArg, EntryCall, ExtLib, ExternDecl, ExternLang, OpaqueType};

fn call_field(name: &str, ns: &str, func: &str, args: Vec<CallArg>) -> EntryField {
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

fn model_of(module: Module) -> crate::ir::Model {
    crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    }
}

/// An `ext` block declaring one extern bound for `langs`. A call field is only
/// well-formed when its `ns.fn` resolves to a declaration like this carrying a
/// binding for the target being generated, so fixtures that expect to clear
/// validation have to supply it.
fn ext_lib_with_extern(lib: &str, name: &str, langs: &[&str]) -> ExtLib {
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
                    errors: vec![],
                    sync: false,
                    infallible: false,
                    ctx: false,
                    receiver: None,
                    is_new: false,
                })
                .collect(),
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
            interface: false,
            instance: None,
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
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: false,
                receiver: None,
                is_new: false,
            })
            .collect(),
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

/// A `call:` line whose receiver is a foreign type name (a static method,
/// `"Type"."method"(args)`) has a rendering in Rust (`krate::Type::method`)
/// and TypeScript (`Type.method` on the imported type) but none in Go, which
/// has no static method to call; Go generation refuses the binding naming
/// the site and the type rather than emitting a method expression that
/// compiles into the wrong call.
#[test]
fn a_static_method_receiver_is_refused_for_go_and_accepted_where_it_renders() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.receiver = Some("Loader".into());
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(
        err.contains("config = ns.load(..)"),
        "names the site: {err}"
    );
    assert!(
        err.contains("Loader.Load"),
        "names the static method: {err}"
    );
    assert!(err.contains("go has no static method"), "{err}");

    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_ok());
}

/// The same refusal at the other place a free extern is called from: a
/// `@header`/`@body` value computed by an extern call in wire position.
#[test]
fn a_static_method_receiver_in_wire_position_is_refused_for_go_only() {
    use crate::ir::{WireCall, WireCallArg};
    let mut module = module_of(vec![]);
    let mut lib = ext_lib_with_extern("ns", "sign", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.receiver = Some("Signer".into());
    }
    module.ext_libs = vec![lib];
    let call = WireCall {
        ns: "ns".into(),
        fn_name: "sign".into(),
        args: vec![WireCallArg::Request],
    };
    let err = super::validate::wire_call_resolves(&module, &call, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("ns.sign(..)"), "{err}");
    assert!(err.contains("Signer.Load"), "{err}");
    assert!(super::validate::wire_call_resolves(&module, &call, &[TargetKind::TypeScript]).is_ok());
    assert!(super::validate::wire_call_resolves(&module, &call, &[TargetKind::Rust]).is_ok());
}

/// A `call:` line passing a declared handle's class itself (`type handle`,
/// for a library that constructs it) has a rendering only in TypeScript,
/// where the imported class is a value. Go and Rust have no type as a
/// value, so generation refuses the binding naming the site and the handle
/// instead of spelling a type where an argument goes.
#[test]
fn a_class_reference_is_refused_for_go_and_rust_and_accepted_in_typescript() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::SymbolCall(crate::ir::SymbolCall {
            symbol: "Pick".into(),
            args: vec![CallArg::TypeRef("answer".into())],
        })];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    for target in [TargetKind::Go, TargetKind::Rust] {
        let err = super::validate_entries(&model, &[target]).unwrap_err();
        assert!(
            err.contains("config = ns.load(..)"),
            "names the site: {err}"
        );
        assert!(err.contains("type answer"), "names the handle: {err}");
        assert!(err.contains("has no class reference to pass"), "{err}");
    }
    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());
}

/// Every target has a string-keyed map to spell a map literal as, so a
/// `call:` line carrying one (at the top level or nested) validates for
/// all three; the capability is still declared per target.
#[test]
fn a_map_literal_is_accepted_in_every_target() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::List(vec![CallArg::Map(vec![(
            "answer".to_string(),
            CallArg::Lit(serde_json::json!(42)),
        )])])];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);
    for target in [TargetKind::Go, TargetKind::Rust, TargetKind::TypeScript] {
        assert!(target.emits_map_literal_args());
        assert!(
            super::validate_entries(&model, &[target]).is_ok(),
            "{target:?}"
        );
    }
}

/// A map literal nested anywhere in the call's own argument tree (a ctor
/// field, a list item, a nested call's argument) is found by the same walk
/// that finds a class reference, so a class reference inside a map is still
/// refused where the target has none.
#[test]
fn a_class_reference_inside_a_map_literal_is_still_refused() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go"]);
    lib.externs[0].langs[0].call_args = vec![CallArg::Map(vec![(
        "impl".to_string(),
        CallArg::TypeRef("answer".into()),
    )])];
    module.ext_libs = vec![lib];
    let model = model_of(module);
    let err = super::validate_entries(&model, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("type answer"), "{err}");
}

/// A wire-position call never applies the binding's `call:` template, so a
/// map literal there would be dropped silently: refused by name in every
/// target, nested inside a list or a nested call included.
#[test]
fn a_map_literal_in_wire_position_is_refused_for_every_target() {
    use crate::ir::{WireCall, WireCallArg};
    let shapes = [
        CallArg::Map(vec![]),
        CallArg::List(vec![CallArg::Map(vec![])]),
        CallArg::SymbolCall(crate::ir::SymbolCall {
            symbol: "Wrap".into(),
            args: vec![CallArg::Map(vec![])],
        }),
        CallArg::Ctor(crate::ir::CallCtor {
            name: "opts".into(),
            fields: [("table".to_string(), CallArg::Map(vec![]))]
                .into_iter()
                .collect(),
        }),
    ];
    for shape in shapes {
        let mut module = module_of(vec![]);
        let mut lib = ext_lib_with_extern("ns", "sign", &["go", "ts", "rust"]);
        for lang in lib.externs[0].langs.iter_mut() {
            lang.call_args = vec![shape.clone()];
        }
        module.ext_libs = vec![lib];
        let call = WireCall {
            ns: "ns".into(),
            fn_name: "sign".into(),
            args: vec![WireCallArg::Request],
        };
        for target in [TargetKind::Go, TargetKind::TypeScript, TargetKind::Rust] {
            let err = super::validate::wire_call_resolves(&module, &call, &[target]).unwrap_err();
            assert!(err.contains("ns.sign(..)"), "{err}");
            assert!(err.contains("map literal"), "{err}");
            assert!(err.contains("wire position"), "{err}");
        }
    }
}

/// A wire-position call passes the trait's own arguments and never applies
/// the binding's `call:` template, so a class reference there would be
/// dropped silently in every target: refused by name, TypeScript included.
#[test]
fn a_class_reference_in_wire_position_is_refused_for_every_target() {
    use crate::ir::{WireCall, WireCallArg};
    let mut module = module_of(vec![]);
    let mut lib = ext_lib_with_extern("ns", "sign", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::TypeRef("signer".into())];
    }
    module.ext_libs = vec![lib];
    let call = WireCall {
        ns: "ns".into(),
        fn_name: "sign".into(),
        args: vec![WireCallArg::Request],
    };
    for target in [TargetKind::Go, TargetKind::TypeScript, TargetKind::Rust] {
        let err = super::validate::wire_call_resolves(&module, &call, &[target]).unwrap_err();
        assert!(err.contains("ns.sign(..)"), "{err}");
        assert!(err.contains("type signer"), "{err}");
        assert!(err.contains("wire position"), "{err}");
    }
}

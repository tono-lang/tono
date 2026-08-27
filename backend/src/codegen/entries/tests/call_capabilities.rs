//! The per-target capability gates over a `call:` line's shapes: a
//! declared position (`#(ctx context.Context)`), a class reference passed
//! as an argument, a method chained on the returned object. Each shape
//! renders in the targets that have it and is refused by name in the
//! others, at generation time, before any emitter could guess. Split out
//! of `call.rs` to stay under the file-size ceiling; the fixtures come
//! from there and from the parent module.

use super::call::{call_field, ext_lib_with_extern, model_of};
use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::CallArg;

/// A declared position (`#(ctx context.Context)`) is what the target binds
/// there, so it must be spelled exactly as Go declares its context, and
/// Rust and TypeScript, which bind nothing, refuse it by name.
#[test]
fn a_declared_position_binds_only_as_go_spells_its_context() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::Foreign("ctx context.Context".into())];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module.clone());

    assert!(super::validate_entries(&model, &[TargetKind::Go]).is_ok());
    let err = super::validate_entries(&model, &[TargetKind::TypeScript]).unwrap_err();
    assert!(
        err.contains("config = ns.load(..)"),
        "names the site: {err}"
    );
    assert!(err.contains("binds no position of its own"), "{err}");
    assert!(super::validate_entries(&model, &[TargetKind::Rust]).is_err());

    module.ext_libs[0].externs[0].langs[0].call_args =
        vec![CallArg::Foreign("c context.Context".into())];
    let err = super::validate_entries(&model_of(module), &[TargetKind::Go]).unwrap_err();
    assert!(
        err.contains("#(ctx context.Context)"),
        "names the expected spelling: {err}"
    );
}

/// The same rule at the other place a free extern is called from: a
/// `@header`/`@body` value computed by an extern call in wire position.
#[test]
fn a_declared_position_in_wire_position_follows_the_same_rule() {
    use crate::ir::{WireCall, WireCallArg};
    let mut module = module_of(vec![]);
    let mut lib = ext_lib_with_extern("ns", "sign", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::Foreign("ctx context.Context".into())];
    }
    module.ext_libs = vec![lib];
    let call = WireCall {
        ns: "ns".into(),
        fn_name: "sign".into(),
        args: vec![WireCallArg::Request],
    };
    assert!(super::validate::wire_call_resolves(&module, &call, &[TargetKind::Go]).is_ok());
    let err =
        super::validate::wire_call_resolves(&module, &call, &[TargetKind::TypeScript]).unwrap_err();
    assert!(err.contains("ns.sign(..)"), "{err}");
    assert!(err.contains("binds no position of its own"), "{err}");
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
    lib.types.push(ts_handle("answer", "Answer"));
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
        assert!(err.contains("\"answer\""), "names the handle: {err}");
        assert!(err.contains("has no class reference to pass"), "{err}");
    }
    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());
}

/// An opaque handle with only a `ts` block naming its class.
fn ts_handle(name: &str, class: &str) -> crate::ir::OpaqueType {
    crate::ir::OpaqueType {
        name: name.into(),
        langs: vec![crate::ir::ForeignLang {
            lang: "ts".into(),
            name: class.into(),
            fields: Default::default(),
        }],
        methods: vec![],
    }
}

/// A wire struct of the module, with the given type parameters.
fn wire_struct(id: &str, params: &[&str]) -> crate::ir::Shape {
    crate::ir::Shape {
        id: id.into(),
        kind: crate::ir::ShapeKind::Structure {
            params: params.iter().map(|p| (*p).to_string()).collect(),
            members: vec![],
        },
        traits: vec![],
    }
}

/// A class reference may also name one of the module's own wire structs:
/// TypeScript gives the struct a runtime class beside its interface, so
/// the binding renders there; Go and Rust refuse it exactly as they refuse
/// a handle's class, naming the struct.
#[test]
fn a_class_reference_to_a_module_struct_renders_in_typescript_and_is_refused_elsewhere() {
    let mut module = module_of(vec![
        entry_shape("m#client", vec![call_field("config", "ns", "load", vec![])]),
        wire_struct("m#profile", &[]),
    ]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.call_args = vec![CallArg::TypeRef("profile".into())];
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    assert!(super::validate_entries(&model, &[TargetKind::TypeScript]).is_ok());
    for target in [TargetKind::Go, TargetKind::Rust] {
        let err = super::validate_entries(&model, &[target]).unwrap_err();
        assert!(err.contains("\"profile\""), "names the struct: {err}");
        assert!(err.contains("has no class reference to pass"), "{err}");
    }
}

/// A class reference with no class behind it is refused for every target,
/// TypeScript included: a name nothing declares (hand-fed IR), a generic
/// struct (no single class to stand as a value), or a shape that is not a
/// wire struct (here an entry).
#[test]
fn a_class_reference_with_no_class_to_pass_is_refused_for_every_target() {
    let cases: Vec<(Vec<crate::ir::Shape>, &str)> = vec![
        (
            vec![],
            "names neither an opaque handle of ext ns nor a struct of module m",
        ),
        (vec![wire_struct("m#profile", &["t"])], "a generic struct"),
        (
            vec![entry_shape("m#profile", vec![])],
            "is not a wire struct",
        ),
    ];
    for (extra, why) in cases {
        let mut shapes = vec![entry_shape(
            "m#client",
            vec![call_field("config", "ns", "load", vec![])],
        )];
        shapes.extend(extra);
        let mut module = module_of(shapes);
        let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
        for lang in lib.externs[0].langs.iter_mut() {
            lang.call_args = vec![CallArg::TypeRef("profile".into())];
        }
        module.ext_libs = vec![lib];
        let model = model_of(module);
        for target in [TargetKind::Go, TargetKind::TypeScript, TargetKind::Rust] {
            let err = super::validate_entries(&model, &[target]).unwrap_err();
            assert!(
                err.contains("config = ns.load(..)"),
                "names the site: {err}"
            );
            assert!(err.contains("\"profile\""), "names the reference: {err}");
            assert!(err.contains(why), "{why}: {err}");
        }
    }
}

/// A `call:` line chaining a method on the returned object
/// (`#(Get)(key).#(Result)()`) renders only in Go, where the chain is one
/// expression with no asynchrony to place. Rust and TypeScript have two
/// calls and one `@async`, so which link is awaited is undeclared:
/// generation refuses the binding naming the site and the chained method.
#[test]
fn a_chained_call_is_refused_for_rust_and_typescript_and_accepted_in_go() {
    let mut module = module_of(vec![entry_shape(
        "m#client",
        vec![call_field("config", "ns", "load", vec![])],
    )]);
    let mut lib = ext_lib_with_extern("ns", "load", &["go", "ts", "rust"]);
    for lang in lib.externs[0].langs.iter_mut() {
        lang.chain = Some(crate::ir::SymbolCall {
            symbol: "Result".into(),
            args: vec![],
        });
    }
    module.ext_libs = vec![lib];
    let model = model_of(module);

    for target in [TargetKind::Rust, TargetKind::TypeScript] {
        let err = super::validate_entries(&model, &[target]).unwrap_err();
        assert!(
            err.contains("config = ns.load(..)"),
            "names the site: {err}"
        );
        assert!(err.contains("#(Result)"), "names the chained method: {err}");
        assert!(err.contains("has no chained call"), "{err}");
    }
    assert!(super::validate_entries(&model, &[TargetKind::Go]).is_ok());
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
        assert!(err.contains("\"signer\""), "{err}");
        assert!(err.contains("wire position"), "{err}");
    }
}

//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! its place in the resolution DAG, its guaranteed classification, its
//! shape dispatch, the `validate_entries` resolution gates, and the
//! ownership rule (reading a method borrows the handle; forwarding it into
//! another call claims it). Split out to keep `mod.rs` under the file-size
//! ceiling; fixtures (`field`, `entry_shape`, `module_of`) come from the
//! parent through `super::*`.

use super::*;
use crate::codegen::fixtures::handle_source::{handle_source_model, handle_source_module};
use crate::codegen::output::TargetKind;
use crate::ir::{CallArg, EntryCall, ExtLib, ExternDecl, ExternLang, OpImplCall, OpaqueType};

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

fn handle_call(recv: &str, method: &str, args: Vec<CallArg>) -> OpImplCall {
    OpImplCall {
        recv: vec![recv.into()],
        method: method.into(),
        args,
    }
}

fn go_lang(symbol: &str) -> ExternLang {
    ExternLang {
        lang: "go".into(),
        symbol: symbol.into(),
        call_args: vec![],
        yields: vec![],
        returns: None,
    }
}

/// `lib#handle` with one `go`-bound method `read` and a `go`-bound
/// constructor `make`.
fn lib_with_handle_method() -> ExtLib {
    ExtLib {
        name: "lib".into(),
        langs: vec![crate::ir::LangPath {
            lang: "go".into(),
            path: "example/lib".into(),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: "handle".into(),
            langs: ["go", "ts", "rust"]
                .into_iter()
                .map(|l| crate::ir::ForeignLang {
                    lang: l.into(),
                    name: if l == "go" {
                        "*Handle".into()
                    } else {
                        "Handle".into()
                    },
                    fields: Default::default(),
                })
                .collect(),
            methods: vec![ExternDecl {
                name: "read".into(),
                params: vec![],
                r#return: Tref::Prim(crate::ir::Prim::String),
                langs: vec![go_lang("Read")],
                r#async: vec![],
                errors: vec![],
            }],
        }],
        externs: vec![ExternDecl {
            name: "make".into(),
            params: vec![],
            r#return: Tref::Ref {
                id: "lib#handle".into(),
                args: vec![],
            },
            langs: vec![go_lang("Make")],
            r#async: vec![],
            errors: vec![],
        }],
    }
}

fn constructed_handle(name: &str) -> EntryField {
    let mut f = field(name, vec![]);
    f.target = Tref::Ref {
        id: "lib#handle".into(),
        args: vec![],
    };
    f.call = Some(EntryCall {
        ns: "lib".into(),
        func: "make".into(),
        args: vec![],
    });
    f
}

fn model_of(fields: Vec<EntryField>) -> crate::ir::Model {
    let mut m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module_of(vec![entry_shape("m#client", fields)])],
    };
    m.modules[0].ext_libs = vec![lib_with_handle_method()];
    m
}

fn sourced(name: &str, recv: &str, method: &str, args: Vec<CallArg>) -> EntryField {
    let mut f = field(name, vec![]);
    f.handle_call = Some(handle_call(recv, method, args));
    f
}

#[test]
fn the_receiver_and_the_arguments_are_resolution_edges() {
    // Declared before its receiver and before the field its argument reads,
    // `value` still resolves after both.
    let value = sourced("value", "handle", "read", vec![call_ref(&["region"])]);
    let region = field("region", vec![Source::Arg]);
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![value, constructed_handle("handle"), region],
    )]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["handle", "region", "value"]);
}

#[test]
fn guaranteed_follows_the_receiver_and_the_arguments() {
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![
            constructed_handle("handle"),
            field("region", vec![Source::Arg]),
            field(
                "maybe",
                vec![Source::Env(crate::ir::EnvName::Name("R".into()))],
            ),
            sourced("plain", "handle", "read", vec![]),
            sourced("with_arg", "handle", "read", vec![call_ref(&["region"])]),
            sourced("with_maybe", "handle", "read", vec![call_ref(&["maybe"])]),
            sourced("orphan", "missing", "read", vec![]),
        ],
    )]);
    let entry = &module_entries(&module)[0];
    let by = |name: &str| {
        entry
            .fields
            .iter()
            .find(|f| f.name == name)
            .copied()
            .unwrap()
    };
    assert!(entry.is_guaranteed(by("plain")));
    assert!(entry.is_guaranteed(by("with_arg")));
    assert!(!entry.is_guaranteed(by("with_maybe")));
    assert!(!entry.is_guaranteed(by("orphan")));
    assert!(matches!(
        entry.field_shape(by("plain"), &module),
        FieldShape::Call
    ));
}

#[test]
fn the_worked_example_validates_for_every_target() {
    for (lang, target) in [
        ("go", TargetKind::Go),
        ("ts", TargetKind::TypeScript),
        ("rust", TargetKind::Rust),
    ] {
        validate_entries(&handle_source_model(lang), &[target])
            .unwrap_or_else(|e| panic!("{lang}: {e}"));
    }
}

#[test]
fn a_call_with_no_receiver_is_refused() {
    let mut f = sourced("value", "handle", "read", vec![]);
    f.handle_call.as_mut().unwrap().recv.clear();
    let err = validate_entries(
        &model_of(vec![constructed_handle("handle"), f]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("no receiver"), "{err}");
}

#[test]
fn an_unknown_receiver_is_refused() {
    let m = model_of(vec![sourced("value", "ghost", "read", vec![])]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("receiver ghost is not a field"), "{err}");
}

#[test]
fn a_receiver_that_is_not_a_handle_is_refused() {
    let m = model_of(vec![
        field("plain", vec![Source::Arg]),
        sourced("value", "plain", "read", vec![]),
    ]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("not a foreign handle field"), "{err}");
}

#[test]
fn an_unknown_method_is_refused() {
    let m = model_of(vec![
        constructed_handle("handle"),
        sourced("value", "handle", "write", vec![]),
    ]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("declares no method named write"), "{err}");
}

#[test]
fn a_method_with_no_block_for_the_target_is_refused() {
    let m = model_of(vec![
        constructed_handle("handle"),
        sourced("value", "handle", "read", vec![]),
    ]);
    let err = validate_entries(&m, &[TargetKind::Rust]).unwrap_err();
    assert!(err.contains("declares no rust block"), "{err}");
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
}

#[test]
fn a_handle_typed_field_cannot_be_sourced_from_a_method() {
    let mut f = sourced("value", "handle", "read", vec![]);
    f.target = Tref::Ref {
        id: "lib#handle".into(),
        args: vec![],
    };
    let m = model_of(vec![constructed_handle("handle"), f]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("is itself a foreign handle"), "{err}");
}

#[test]
fn a_config_member_sourced_from_a_method_is_refused() {
    let mut m = model_of(vec![constructed_handle("handle")]);
    let mut member = sourced("value", "handle", "read", vec![]);
    member.target = Tref::Prim(crate::ir::Prim::String);
    m.modules[0].shapes.push(Shape {
        id: "m#settings".into(),
        kind: ShapeKind::Config {
            fields: vec![member],
        },
        traits: vec![],
    });
    let mut composed = field("settings", vec![]);
    composed.target = Tref::Ref {
        id: "m#settings".into(),
        args: vec![],
    };
    if let ShapeKind::Entry { fields, .. } = &mut m.modules[0].shapes[0].kind {
        fields.push(composed);
    }
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(
        err.contains("config member has no handle in scope"),
        "{err}"
    );
}

/// Reading a method borrows the handle: a field sourced from it, an op's
/// own `impl` body reading it, and a second field sourced from it all
/// coexist (this is the shape the compiled fixtures use).
#[test]
fn reading_a_method_coexists_with_every_other_read() {
    let module = handle_source_module("go");
    let m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
}

/// The handle forwarded into another call (`relay = lib.make(.handle)`) is
/// owned by that call; a field reading one of its methods is a second
/// reader and is refused, in either declaration order.
#[test]
fn a_forwarded_handle_also_read_by_a_field_source_is_refused_in_both_orders() {
    let mut relay = constructed_handle("relay");
    relay.call.as_mut().unwrap().args = vec![call_ref(&["handle"])];
    let reader = sourced("value", "handle", "read", vec![]);
    for fields in [
        vec![constructed_handle("handle"), relay.clone(), reader.clone()],
        vec![constructed_handle("handle"), reader.clone(), relay.clone()],
    ] {
        let err = validate_entries(&model_of(fields), &[TargetKind::Go]).unwrap_err();
        assert!(err.contains("relay = lib.make(..)"), "{err}");
        assert!(err.contains("value = .handle.read(..)"), "{err}");
    }
}

/// A field-position call can itself forward a sibling handle through its
/// arguments, and then owns it like any other forwarding call: an op
/// reading that handle afterwards is refused.
#[test]
fn a_field_source_forwarding_a_handle_owns_it() {
    let mut m = model_of(vec![
        constructed_handle("handle"),
        constructed_handle("other"),
        sourced("value", "handle", "read", vec![call_ref(&["other"])]),
    ]);
    if let ShapeKind::Entry { operations, .. } = &mut m.modules[0].shapes[0].kind {
        operations.push(Shape {
            id: "m#client.ping".into(),
            kind: ShapeKind::Operation {
                input_name: None,
                input: None,
                output: None,
                output_nullable: false,
                errors: vec![],
                wire: None,
                impl_call: Some(handle_call("other", "read", vec![])),
            },
            traits: vec![],
        });
    }
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("value = .handle.read(..)"), "{err}");
    assert!(err.contains("operation ping impl .other.read(..)"), "{err}");
}

/// A field-position call forwarding an injected handle (no construction
/// call of its own) is refused like a free call doing the same.
#[test]
fn a_field_source_forwarding_an_injected_handle_is_refused() {
    let mut injected = field("injected", vec![Source::Arg]);
    injected.target = Tref::Ref {
        id: "lib#handle".into(),
        args: vec![],
    };
    let m = model_of(vec![
        constructed_handle("handle"),
        injected,
        sourced("value", "handle", "read", vec![call_ref(&["injected"])]),
    ]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("an injected foreign handle"), "{err}");
}

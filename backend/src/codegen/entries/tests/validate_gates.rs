//! `validate_entries` gate tests split out to keep `mod.rs` under the
//! file-size ceiling; fixtures (`field`, `entry_shape`, `module_of`) come
//! from the parent module via `super::*`.

use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::{
    CallArg, CallCtor, EntryCall, ExtLib, ExternDecl, ExternLang, OpaqueType, WireValue,
};

#[test]
fn validation_rejects_the_cases_no_layer_would_diagnose() {
    let model = |shapes: Vec<Shape>, extensions: Vec<crate::ir::Extension>| crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![Module {
            tests: vec![],
            name: "m".into(),
            shapes,
            operations: vec![],
            extensions,
            ext_libs: vec![],
        }],
    };
    // A field named after a transport slot collides with the Settings member.
    let err = validate_entries(
        &model(
            vec![entry_shape(
                "m#client",
                vec![field("headers", vec![Source::Arg])],
            )],
            vec![],
        ),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("transport slot"), "{err}");
    // A loose op and an entry op sharing a local name would collide.
    let entry_op = Shape {
        id: "m#sdk.save".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits: vec![],
    };
    let loose_op = Shape {
        id: "m#save".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits: vec![],
    };
    let mut m = model(
        vec![Shape {
            id: "m#sdk".into(),
            kind: ShapeKind::Entry {
                fields: vec![],
                operations: vec![entry_op],
            },
            traits: vec![],
        }],
        vec![],
    );
    m.modules[0].operations = vec![loose_op];
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("declared both loose and in entry"), "{err}");
    // An entry named client next to loose operations collides with the
    // Client interface they emit.
    let extra_op = Shape {
        id: "m#other".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits: vec![],
    };
    let mut m = model(vec![entry_shape("m#client", vec![])], vec![]);
    m.modules[0].operations = vec![extra_op];
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("Client interface"), "{err}");
    // A construction field referencing a shape outside the module is
    // rejected with the offending id named.
    let mut cross = field("creds", vec![]);
    cross.target = Tref::Ref {
        id: "other#credentials".into(),
        args: vec![],
    };
    let err = validate_entries(
        &model(vec![entry_shape("m#client", vec![cross])], vec![]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("other#credentials"), "{err}");
    // An @arg named after a constructor local shadows it in the generated
    // signature.
    let err = validate_entries(
        &model(
            vec![entry_shape(
                "m#client",
                vec![field("config", vec![Source::Arg])],
            )],
            vec![],
        ),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("local the generated constructor"), "{err}");
    // A non-arg field may use those names freely (it only lives as s.<field>).
    assert!(validate_entries(
        &model(
            vec![entry_shape(
                "m#client",
                vec![field(
                    "config",
                    vec![Source::Env(EnvName::Name("C".into()))]
                )],
            )],
            vec![],
        ),
        &[TargetKind::Go]
    )
    .is_ok());
    // A sibling spelling a derived why/set variable collides with it.
    let err = validate_entries(
        &model(
            vec![entry_shape(
                "m#client",
                vec![
                    field("endpoint", vec![Source::Env(EnvName::Name("E".into()))]),
                    field("endpoint_why", vec![Source::Arg]),
                ],
            )],
            vec![],
        ),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("endpoint_why"), "{err}");
    // An entry op with an @http binding that names no endpoint: the frontend
    // enforces this, but IR read from a file or stdin never went through it.
    let no_endpoint_op = Shape {
        id: "m#sdk.get".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: Some(Box::new(crate::ir::WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
                body: None,
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: None,
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
            impl_call: None,
        },
        traits: vec![],
    };
    let err = validate_entries(
        &model(
            vec![Shape {
                id: "m#sdk".into(),
                kind: ShapeKind::Entry {
                    fields: vec![],
                    operations: vec![no_endpoint_op],
                },
                traits: vec![],
            }],
            vec![],
        ),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("no endpoint"), "{err}");
    // A single entry named new collides with the Go constructor.
    let err = validate_entries(
        &model(vec![entry_shape("m#new", vec![])], vec![]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("New constructor"), "{err}");
    // In a multi-entry module, new_<entry> spells the other entry's
    // constructor name.
    let err = validate_entries(
        &model(
            vec![
                entry_shape("m#admin", vec![]),
                entry_shape("m#new_admin", vec![]),
            ],
            vec![],
        ),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("NewAdmin"), "{err}");
    // An @arg whose canonical name is clean but whose @rename(go) override is a
    // keyword is rejected on the rendered parameter, not just the canonical name.
    let mut renamed = field("kind", vec![Source::Arg]);
    renamed.traits = vec![crate::ir::Trait {
        id: "rename".into(),
        value: json!({"go": "type"}),
    }];
    let err = validate_entries(
        &model(vec![entry_shape("m#client", vec![renamed])], vec![]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("keyword") && err.contains("type"), "{err}");
    // A clean single-entry module passes.
    assert!(validate_entries(
        &model(
            vec![entry_shape(
                "m#client",
                vec![field("token", vec![Source::Arg])]
            )],
            vec![],
        ),
        &[TargetKind::Go]
    )
    .is_ok());
}

#[test]
fn has_entries_sees_only_entry_shapes() {
    assert!(!has_entries(&module_of(vec![])));
    assert!(has_entries(&module_of(vec![entry_shape("m#c", vec![])])));
}

#[test]
fn validation_rejects_shapes_and_args_spelling_generated_identifiers() {
    let model = |shapes: Vec<Shape>| crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![Module {
            tests: vec![],
            name: "m".into(),
            shapes,
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    };
    // A wire struct spelling a generated companion type collides with it.
    let settings = Shape {
        id: "m#Settings".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    };
    let err = validate_entries(
        &model(vec![entry_shape("m#client", vec![]), settings]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("settings companion"), "{err}");
    // The same shape in its canonical (snake) spelling, which is how the
    // frontend actually names it, collides just the same: the comparison is
    // on the emitted type identifier, not the raw id.
    let settings_snake = Shape {
        id: "m#settings".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    };
    let err = validate_entries(
        &model(vec![entry_shape("m#client", vec![]), settings_snake]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("settings companion"), "{err}");
    // A shape spelling the entry's own client type collides too.
    let client_type = Shape {
        id: "m#Client".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    };
    let err = validate_entries(
        &model(vec![entry_shape("m#client", vec![]), client_type]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("client type"), "{err}");
    // An @arg named after a target-language keyword is an invalid parameter.
    let err = validate_entries(
        &model(vec![entry_shape(
            "m#client",
            vec![field("type", vec![Source::Arg])],
        )]),
        &[TargetKind::Go],
    )
    .unwrap_err();
    assert!(err.contains("keyword"), "{err}");
}

#[test]
fn a_loose_operation_with_a_wire_binding_is_rejected() {
    // A loose (non-entry) operation carrying a wire binding is rejected
    // outright: entries are the only supported HTTP client surface.
    let mut m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![Module {
            tests: vec![],
            name: "m".into(),
            shapes: vec![entry_shape("m#sdk", vec![])],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    };
    let wire_loose_op = Shape {
        id: "m#ping".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: Some(Box::new(crate::ir::WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![crate::ir::TemplatePart::Lit("/ping".into())]),
                body: None,
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: None,
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
            impl_call: None,
        },
        traits: vec![],
    };
    m.modules[0].operations = vec![wire_loose_op];
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("outside an entry"), "{err}");
}

/// An entry op whose own implementation is a call into a foreign handle's
/// method (`impl .bus.send(..)`), named after the entry, the op, the field,
/// and the method; `bus` is an injected `bus#publisher` handle field.
fn entry_with_handle_call() -> Shape {
    let op = Shape {
        id: "m#client.publish".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: Some(crate::ir::OpImplCall {
                recv: vec!["bus".into()],
                method: "send".into(),
                args: vec![],
            }),
        },
        traits: vec![],
    };
    Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![handle_field("bus", "bus", "publisher", vec![Source::Arg])],
            operations: vec![op],
        },
        traits: vec![],
    }
}

/// The `bus` lib behind [`entry_with_handle_call`]: one `publisher` handle
/// whose `send` method is bound for each of `langs`.
fn bus_lib_with_send_bound_for(langs: &[&str]) -> ExtLib {
    ExtLib {
        name: "bus".into(),
        langs: vec![],
        structs: vec![],
        types: vec![OpaqueType {
            name: "publisher".into(),
            interface: false,
            instance: None,
            methods: vec![ExternDecl {
                name: "send".into(),
                params: vec![],
                r#return: Tref::Prim(crate::ir::Prim::String),
                langs: langs
                    .iter()
                    .map(|lang| ExternLang {
                        lang: (*lang).into(),
                        symbol: "send".into(),
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
        }],
        externs: vec![],
    }
}

#[test]
fn every_target_now_accepts_an_op_own_extern_handle_call() {
    let mut module = module_of(vec![entry_with_handle_call()]);
    module.ext_libs = vec![bus_lib_with_send_bound_for(&["go", "ts", "rust"])];
    let m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    // Go, TypeScript and Rust all emit an op's own extern handle-method call
    // now, so no combination of the three targets trips the
    // `emits_ext_handle_calls` gate below; that gate stays in place for a
    // future target that lands the plain call/handle-type halves before
    // this one.
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
    assert!(validate_entries(&m, &[TargetKind::TypeScript]).is_ok());
    assert!(validate_entries(&m, &[TargetKind::Rust]).is_ok());
    assert!(validate_entries(
        &m,
        &[TargetKind::Go, TargetKind::TypeScript, TargetKind::Rust]
    )
    .is_ok());
}

/// An op's own `impl .bus.send(..)` whose method lacks a binding for a
/// requested target is refused by name at generation time (the same
/// resolution a `= .bus.send(..)` field source gets), instead of the
/// target's emitter meeting the missing block as a pipeline defect.
#[test]
fn an_op_own_handle_call_whose_method_is_unbound_for_a_target_is_refused_by_name() {
    let mut module = module_of(vec![entry_with_handle_call()]);
    module.ext_libs = vec![bus_lib_with_send_bound_for(&["go"])];
    let m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
    for target in [TargetKind::TypeScript, TargetKind::Rust] {
        let err = validate_entries(&m, &[TargetKind::Go, target]).unwrap_err();
        assert!(
            err.contains("operation client.publish impl .bus.send(..)")
                && err.contains("extern send declares no"),
            "{err}"
        );
    }
    // The receiver must be a foreign-handle field of the entry, and the
    // method one the handle declares.
    let mut orphan = module_of(vec![entry_with_handle_call()]);
    orphan.ext_libs = vec![];
    let orphan = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![orphan],
    };
    let err = validate_entries(&orphan, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("not a foreign handle field"), "{err}");
}

/// A handle lib declaring one opaque type (with a Go-bound `ping` method,
/// the one the ownership cases' op reads) and one free constructor that
/// returns it, so a field can either construct the handle (`field.call`) or
/// be injected as one (`@arg`, no `call`).
fn ext_lib_with_handle_constructor(lib: &str, handle: &str, ctor: &str) -> ExtLib {
    ExtLib {
        name: lib.into(),
        langs: vec![],
        structs: vec![],
        types: vec![OpaqueType {
            name: handle.into(),
            interface: false,
            instance: None,
            methods: vec![ExternDecl {
                name: "ping".into(),
                params: vec![],
                r#return: Tref::Prim(crate::ir::Prim::String),
                langs: vec![ExternLang {
                    lang: "go".into(),
                    symbol: "Ping".into(),
                    call_args: vec![],
                    yields: vec![],
                    returns: None,
                    errors: vec![],
                    sync: false,
                    infallible: false,
                    ctx: false,
                    receiver: None,
                    is_new: false,
                }],
            }],
        }],
        externs: vec![ExternDecl {
            name: ctor.into(),
            params: vec![],
            r#return: Tref::Ref {
                id: format!("{lib}#{handle}"),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Make".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: false,
                receiver: None,
                is_new: false,
            }],
        }],
    }
}

fn handle_field(name: &str, lib: &str, handle: &str, sources: Vec<Source>) -> EntryField {
    let mut f = field(name, sources);
    f.target = Tref::Ref {
        id: format!("{lib}#{handle}"),
        args: vec![],
    };
    f
}

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

/// A two-field entry (`source`, plus `combined` whose own construction call
/// forwards `combined_args`) over the shared `lib#handle` fixture: every
/// case below differs only in `source`'s own sourcing and in how the
/// forwarding argument nests a `Ref` to it, so this is the one place that
/// shape is spelled out.
fn model_with_forwarding_call(source: EntryField, combined_args: Vec<CallArg>) -> crate::ir::Model {
    let mut combined = handle_field("combined", "lib", "handle", vec![]);
    combined.call = Some(EntryCall {
        ns: "lib".into(),
        func: "make".into(),
        args: combined_args,
    });
    let mut m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module_of(vec![entry_shape(
            "m#client",
            vec![source, combined],
        )])],
    };
    m.modules[0].ext_libs = vec![ext_lib_with_handle_constructor("lib", "handle", "make")];
    m
}

/// A `lib#handle` field this generator itself constructs (`primary =
/// lib.make()`), the forwardable counterpart of [`injected_source`].
fn constructed_primary() -> EntryField {
    let mut primary = handle_field("primary", "lib", "handle", vec![]);
    primary.call = Some(EntryCall {
        ns: "lib".into(),
        func: "make".into(),
        args: vec![],
    });
    primary
}

fn injected_source() -> EntryField {
    handle_field("injected", "lib", "handle", vec![Source::Arg])
}

/// An `@arg`-injected handle field (`injected`, no `call` of its own) passed
/// as another field's own construction-call argument (`combined`'s
/// `make(.injected)`): rejected up front, naming the injected field and the
/// call it was forwarded into, rather than reaching a target's emitter
/// (which would either unwrap an unchecked type assertion that panics at
/// runtime on a caller-supplied value, or leave a plain interface reference
/// that fails `go build` on a type the author never wrote).
#[test]
fn an_injected_handle_forwarded_into_another_call_is_named_and_refused() {
    let m = model_with_forwarding_call(injected_source(), vec![call_ref(&["injected"])]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("combined"), "{err}");
    assert!(err.contains("injected"), "{err}");
    assert!(err.contains("lib.make(..)"), "{err}");
}

/// The same shape, but `combined` forwards a sibling handle field this
/// generator itself constructed (`primary.call` is `Some`, not injected):
/// the guard only fires on an injected source, so a fully-constructed chain
/// still validates.
#[test]
fn a_constructed_handle_forwarded_into_another_call_still_validates() {
    let primary = constructed_primary();
    let m = model_with_forwarding_call(primary, vec![call_ref(&["primary"])]);
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
}

/// The same injected-handle-forwarded shape, but the `Ref` reaches the
/// injected field nested inside a struct-literal argument (`opts { x:
/// .injected }`) rather than as a call argument directly: `ref_paths` must
/// recurse into a `Ctor`'s own fields, not just walk the call's top-level
/// argument list.
#[test]
fn an_injected_handle_nested_in_a_ctor_argument_is_still_refused() {
    let ctor_arg = CallArg::Ctor(CallCtor {
        name: "opts".into(),
        fields: std::collections::BTreeMap::from([("x".to_string(), call_ref(&["injected"]))]),
    });
    let m = model_with_forwarding_call(injected_source(), vec![ctor_arg]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("combined"), "{err}");
    assert!(err.contains("injected"), "{err}");
}

/// Same shape again, but the `Ref` reaches the injected field nested inside
/// a `List` argument: `ref_paths` must recurse into a `List`'s own items
/// too, not just a `Ctor`'s fields.
#[test]
fn an_injected_handle_nested_in_a_list_argument_is_still_refused() {
    let list_arg = CallArg::List(vec![call_ref(&["injected"])]);
    let m = model_with_forwarding_call(injected_source(), vec![list_arg]);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("combined"), "{err}");
    assert!(err.contains("injected"), "{err}");
}

/// A constructed handle (`primary`) forwarded into `combined`'s call, plus
/// an op whose own `impl` call names `primary` as `recv` (or as an argument
/// when `as_arg`): the model the ownership gate grades.
fn model_with_forwarded_and_op_read(as_arg: bool) -> crate::ir::Model {
    let primary = constructed_primary();
    let mut m = model_with_forwarding_call(primary, vec![call_ref(&["primary"])]);
    let (recv, args) = if as_arg {
        (vec!["combined".to_string()], vec![call_ref(&["primary"])])
    } else {
        (vec!["primary".to_string()], vec![])
    };
    let op = crate::ir::Shape {
        id: "m#client.ping".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: Some(crate::ir::OpImplCall {
                recv,
                method: "ping".into(),
                args,
            }),
        },
        traits: vec![],
    };
    if let ShapeKind::Entry { operations, .. } = &mut m.modules[0].shapes[0].kind {
        operations.push(op);
    }
    m
}

/// A constructed handle handed on to another call is owned by that call:
/// an op's own `impl .primary.ping()` reading the same handle afterwards
/// would compile in Rust and then diagnose "not configured" at runtime for
/// a handle the caller did configure (the slot was moved out), while a
/// reference-semantics target would alias it. Refused up front, naming the
/// forwarding call, the handle and the second reader.
#[test]
fn a_forwarded_handle_also_read_as_an_op_receiver_is_refused() {
    let m = model_with_forwarded_and_op_read(false);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("combined = lib.make(..)"), "{err}");
    assert!(err.contains("handle primary"), "{err}");
    assert!(
        err.contains("operation ping impl .primary.ping(..)"),
        "{err}"
    );
}

/// The same handle read as an argument of an op's `impl` call instead of
/// its receiver is the same second read.
#[test]
fn a_forwarded_handle_also_read_as_an_op_argument_is_refused() {
    let m = model_with_forwarded_and_op_read(true);
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("handle primary"), "{err}");
    assert!(
        err.contains("operation ping impl .combined.ping(.primary)"),
        "{err}"
    );
}

/// Two calls each forwarding the same constructed handle: the second call
/// would find the slot already moved out. Refused naming both calls.
#[test]
fn a_handle_forwarded_into_two_calls_is_refused() {
    let primary = constructed_primary();
    let mut m = model_with_forwarding_call(primary, vec![call_ref(&["primary"])]);
    let mut second = handle_field("second", "lib", "handle", vec![]);
    second.call = Some(EntryCall {
        ns: "lib".into(),
        func: "make".into(),
        args: vec![call_ref(&["primary"])],
    });
    if let ShapeKind::Entry { fields, .. } = &mut m.modules[0].shapes[0].kind {
        fields.push(second);
    }
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("combined = lib.make(..)"), "{err}");
    assert!(err.contains("second = lib.make(..)"), "{err}");
}

/// The op reads the *constructed* handle (`combined`), not the one that
/// was forwarded into it: the forwarded handle has its one reader and the
/// spec validates. This is the shape the compiled Rust fixture uses.
#[test]
fn an_op_reading_the_handle_built_from_a_forwarded_one_still_validates() {
    let mut m = model_with_forwarded_and_op_read(false);
    if let ShapeKind::Entry { operations, .. } = &mut m.modules[0].shapes[0].kind {
        if let ShapeKind::Operation {
            impl_call: Some(call),
            ..
        } = &mut operations[0].kind
        {
            call.recv = vec!["combined".into()];
        }
    }
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
}

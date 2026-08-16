//! `validate_entries` gate tests split out to keep `mod.rs` under the
//! file-size ceiling; fixtures (`field`, `entry_shape`, `module_of`) come
//! from the parent module via `super::*`.

use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::WireValue;

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
/// and the method.
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
            fields: vec![],
            operations: vec![op],
        },
        traits: vec![],
    }
}

#[test]
fn a_target_that_cannot_emit_an_extern_handle_call_is_named_and_refused() {
    let m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module_of(vec![entry_with_handle_call()])],
    };
    // Go emits an op's own extern handle-method call today; TypeScript and
    // Rust do not, and a request naming either alongside Go must refuse the
    // whole call, not silently drop it from just the targets that cannot
    // render it.
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());
    let err = validate_entries(&m, &[TargetKind::TypeScript]).unwrap_err();
    assert!(err.contains("client.publish"), "{err}");
    assert!(err.contains(".bus.send(..)"), "{err}");
    assert!(
        err.contains("typescript cannot emit that call yet"),
        "{err}"
    );
    let err = validate_entries(&m, &[TargetKind::Go, TargetKind::Rust]).unwrap_err();
    assert!(err.contains("rust cannot emit that call yet"), "{err}");
}

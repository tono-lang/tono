//! `validate_entries` gate tests split out to keep `mod.rs` under the
//! file-size ceiling; fixtures (`field`, `entry_shape`, `module_of`) come
//! from the parent module via `super::*`.

use super::*;
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
        true,
    )
    .unwrap_err();
    assert!(err.contains("transport slot"), "{err}");
    // client_init cannot bridge two Settings types.
    let hook = crate::ir::Extension {
        name: "client_init".into(),
        kind: crate::ir::ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("go".to_string(), "ext/go/i.go#I".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    };
    let err = validate_entries(
        &model(
            vec![
                entry_shape("m#client", vec![]),
                entry_shape("m#admin", vec![]),
            ],
            vec![hook],
        ),
        true,
    )
    .unwrap_err();
    assert!(err.contains("client_init"), "{err}");
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
    let err = validate_entries(&m, true).unwrap_err();
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
    let err = validate_entries(&m, true).unwrap_err();
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
        true,
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
        true,
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
        true
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
        true,
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
        true,
    )
    .unwrap_err();
    assert!(err.contains("no endpoint"), "{err}");
    // A single entry named new collides with the Go constructor.
    let err =
        validate_entries(&model(vec![entry_shape("m#new", vec![])], vec![]), true).unwrap_err();
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
        true,
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
        true,
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
        true
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
        true,
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
        true,
    )
    .unwrap_err();
    assert!(err.contains("client type"), "{err}");
    // An @arg named after a target-language keyword is an invalid parameter.
    let err = validate_entries(
        &model(vec![entry_shape(
            "m#client",
            vec![field("type", vec![Source::Arg])],
        )]),
        true,
    )
    .unwrap_err();
    assert!(err.contains("keyword"), "{err}");
}

#[test]
fn a_mixed_module_with_a_ts_client_init_binding_is_rejected() {
    let hook = crate::ir::Extension {
        name: "client_init".into(),
        kind: crate::ir::ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("ts".to_string(), "ext/ts/i.ts#init".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    };
    let loose_op = Shape {
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
    let mut m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![Module {
            tests: vec![],
            name: "m".into(),
            shapes: vec![entry_shape("m#sdk", vec![])],
            operations: vec![loose_op],
            extensions: vec![hook],
            ext_libs: vec![],
        }],
    };
    let err = validate_entries(&m, true).unwrap_err();
    assert!(err.contains("mixes loose operations"), "{err}");
    m.modules[0].extensions.clear();
    m.modules[0].operations.clear();
    assert!(validate_entries(&m, true).is_ok());
    // A loose (non-entry) operation carrying a wire binding is rejected
    // outright: entries are the only supported HTTP client surface.
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
    let err = validate_entries(&m, true).unwrap_err();
    assert!(err.contains("outside an entry"), "{err}");
}

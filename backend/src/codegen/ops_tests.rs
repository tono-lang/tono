use super::*;
use crate::ir::{Shape, ShapeKind};
use serde_json::json;

fn trait_of(id: &str, value: serde_json::Value) -> Trait {
    Trait {
        id: id.into(),
        value,
    }
}

fn op(traits: Vec<Trait>, errors: Vec<&str>) -> Shape {
    Shape {
        id: "m#do_thing".into(),
        kind: ShapeKind::Operation {
            input: None,
            output: None,
            errors: errors
                .into_iter()
                .map(|id| Tref::Ref {
                    id: id.into(),
                    args: vec![],
                })
                .collect(),
            wire: None,
        },
        traits,
    }
}

fn error_shape(id: &str, traits: Vec<Trait>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits,
    }
}

/// An error shape with a status and an `@errorCode(path, value)`, for the
/// generation-time gate tests.
fn coded_error_shape(id: &str, status: i64, path: &str, value: &str) -> Shape {
    error_shape(
        id,
        vec![
            trait_of("status", json!([status])),
            trait_of("errorCode", json!([path, value])),
        ],
    )
}

/// A single-module model wrapping `shapes` and `op`, for `validate_error_codes`.
fn model_of(shapes: Vec<Shape>, op: Shape) -> crate::ir::Model {
    crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module(shapes, vec![op])],
    }
}

fn module(shapes: Vec<Shape>, operations: Vec<Shape>) -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes,
        operations,
        extensions: vec![],
    }
}

#[test]
fn a_declared_error_never_used_as_data_is_error_only() {
    let m = module(
        vec![error_shape("m#not_found", vec![])],
        vec![op(vec![], vec!["m#not_found"])],
    );
    let error_only = error_only_shapes(&m);
    assert!(error_only.contains("m#not_found"));
}

#[test]
fn a_declared_error_also_used_as_an_operation_input_is_not_error_only() {
    let mut input_op = op(vec![], vec!["m#retry_hint"]);
    input_op.kind = ShapeKind::Operation {
        input: Some(Tref::Ref {
            id: "m#retry_hint".into(),
            args: vec![],
        }),
        output: None,
        errors: vec![Tref::Ref {
            id: "m#retry_hint".into(),
            args: vec![],
        }],
        wire: None,
    };
    let m = module(vec![error_shape("m#retry_hint", vec![])], vec![input_op]);
    assert!(error_only_shapes(&m).is_empty());
}

#[test]
fn a_declared_error_also_used_as_a_member_type_is_not_error_only() {
    let holder = Shape {
        id: "m#holder".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![crate::codegen::test_support::member(
                "cause",
                Tref::Ref {
                    id: "m#not_found".into(),
                    args: vec![],
                },
                true,
            )],
        },
        traits: vec![],
    };
    let m = module(
        vec![error_shape("m#not_found", vec![]), holder],
        vec![op(vec![], vec!["m#not_found"])],
    );
    assert!(error_only_shapes(&m).is_empty());
}

fn entry_field(name: &str, target_id: &str) -> crate::ir::EntryField {
    crate::ir::EntryField {
        name: name.into(),
        target: Tref::Ref {
            id: target_id.into(),
            args: vec![],
        },
        sources: vec![],
        format: None,
        transforms: vec![],
        select: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

#[test]
fn a_declared_error_also_used_as_a_nested_entry_ops_input_is_not_error_only() {
    let mut nested_op = op(vec![], vec!["m#not_found"]);
    nested_op.id = "m#client.do_thing".into();
    nested_op.kind = ShapeKind::Operation {
        input: Some(Tref::Ref {
            id: "m#not_found".into(),
            args: vec![],
        }),
        output: None,
        errors: vec![Tref::Ref {
            id: "m#not_found".into(),
            args: vec![],
        }],
        wire: None,
    };
    let entry = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![],
            operations: vec![nested_op],
        },
        traits: vec![],
    };
    let m = module(vec![error_shape("m#not_found", vec![]), entry], vec![]);
    assert!(error_only_shapes(&m).is_empty());
}

#[test]
fn a_declared_error_also_used_as_an_entry_or_config_fields_target_is_not_error_only() {
    let wire_op = |errors: Vec<&str>| {
        let mut o = op(vec![], errors);
        o.id = "m#client.ping".into();
        o
    };
    let entry = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![entry_field("hint", "m#not_found")],
            operations: vec![wire_op(vec!["m#not_found"])],
        },
        traits: vec![],
    };
    let m = module(vec![error_shape("m#not_found", vec![]), entry], vec![]);
    assert!(error_only_shapes(&m).is_empty());

    let cfg = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config {
            fields: vec![entry_field("hint", "m#slow_down")],
        },
        traits: vec![],
    };
    let entry2 = Shape {
        id: "m#client2".into(),
        kind: ShapeKind::Entry {
            fields: vec![],
            operations: vec![wire_op(vec!["m#slow_down"])],
        },
        traits: vec![],
    };
    let m2 = module(
        vec![error_shape("m#slow_down", vec![]), cfg, entry2],
        vec![],
    );
    assert!(error_only_shapes(&m2).is_empty());
}

#[test]
fn the_async_trait_is_authoritative_and_transport_infers_async() {
    // Explicit @async, with or without a transport, is async.
    assert_eq!(
        effect_of(&op(vec![trait_of("async", json!(null))], vec![])),
        Effect::Async
    );
    // A transport binding alone infers async.
    assert_eq!(
        effect_of(&op(
            vec![trait_of("http", json!({"method": "POST"}))],
            vec![]
        )),
        Effect::Async
    );
    // A purely local operation is sync.
    assert_eq!(effect_of(&op(vec![], vec![])), Effect::Sync);
    // The namespaced spelling counts too.
    assert_eq!(
        effect_of(&op(vec![trait_of("async", json!(null))], vec![])),
        Effect::Async
    );
}

#[test]
fn declared_errors_resolve_status_code_and_retryable() {
    let module = module(
        vec![
            error_shape(
                "m#payment_declined",
                vec![
                    trait_of("status", json!([402])),
                    trait_of("errorCode", json!(["code", "payment_declined"])),
                    trait_of("retryable", json!(null)),
                ],
            ),
            error_shape("m#rate_limited", vec![trait_of("status", json!(429))]),
        ],
        vec![],
    );
    let op = op(vec![], vec!["m#payment_declined", "m#rate_limited"]);
    let errors = declared_errors(&op, &module);
    assert_eq!(
        errors,
        vec![
            DeclaredError {
                shape_id: "m#payment_declined".into(),
                status: Some(402),
                code: Some(ErrorCode {
                    path: vec!["code".into()],
                    value: "payment_declined".into(),
                }),
                retryable: true,
            },
            DeclaredError {
                shape_id: "m#rate_limited".into(),
                status: Some(429),
                code: None,
                retryable: false,
            },
        ]
    );
}

#[test]
fn unresolved_and_repeated_references_are_skipped() {
    let module = module(
        vec![error_shape(
            "m#not_found",
            vec![trait_of("status", json!([404]))],
        )],
        vec![],
    );
    let op = op(vec![], vec!["m#not_found", "m#nope", "m#not_found"]);
    let errors = declared_errors(&op, &module);
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].shape_id, "m#not_found");
}

#[test]
fn module_errors_are_the_union_in_first_appearance_order() {
    let shapes = vec![
        error_shape("m#a", vec![trait_of("status", json!([400]))]),
        error_shape("m#b", vec![trait_of("status", json!([404]))]),
    ];
    let op_one = op(vec![], vec!["m#b", "m#a"]);
    let op_two = op(vec![], vec!["m#a"]);
    let module = module(shapes, vec![op_one, op_two]);
    let ids: Vec<String> = module_declared_errors(&module)
        .into_iter()
        .map(|e| e.shape_id)
        .collect();
    assert_eq!(ids, vec!["m#b".to_string(), "m#a".to_string()]);
}

#[test]
fn the_discriminator_driver_skips_operations_with_no_declared_errors() {
    let shapes = vec![error_shape("m#nf", vec![trait_of("status", json!([404]))])];
    let with_errors = op(vec![], vec!["m#nf"]);
    let without = op(vec![], vec![]);
    let module = module(shapes, vec![without, with_errors]);
    let decls = discriminator_decls(&module, |_, ordered| {
        crate::codegen::tree::Decl::raw(format!("{} entries", ordered.len()))
    });
    // Only the error-declaring operation gets a declaration.
    assert_eq!(decls.len(), 1);
}

#[test]
fn discrimination_tries_coded_entries_before_the_codeless_catch_all() {
    let shapes = vec![
        error_shape("m#generic_bad", vec![trait_of("status", json!([400]))]),
        error_shape(
            "m#coded_bad",
            vec![
                trait_of("status", json!([400])),
                trait_of("errorCode", json!(["code", "specific"])),
            ],
        ),
        // A shape the frontend would have rejected: no status. It never
        // enters the discrimination map.
        error_shape("m#no_status", vec![]),
    ];
    let op = op(vec![], vec!["m#generic_bad", "m#coded_bad", "m#no_status"]);
    let module = module(shapes, vec![]);
    let ordered: Vec<String> = discrimination_order(&op, &module)
        .into_iter()
        .map(|e| e.shape_id)
        .collect();
    assert_eq!(
        ordered,
        vec!["m#coded_bad".to_string(), "m#generic_bad".to_string()]
    );
}

#[test]
fn error_code_splits_a_dotted_path_into_segments() {
    let module = module(
        vec![error_shape(
            "m#invalid_request",
            vec![
                trait_of("status", json!([400])),
                trait_of("errorCode", json!(["error.type", "invalid"])),
            ],
        )],
        vec![],
    );
    let errors = declared_errors(&op(vec![], vec!["m#invalid_request"]), &module);
    assert_eq!(
        errors[0].code,
        Some(ErrorCode {
            path: vec!["error".into(), "type".into()],
            value: "invalid".into(),
        })
    );
}

#[test]
fn a_malformed_error_code_resolves_to_no_code() {
    // Wrong arity or a non-string element: no code, the same as an absent
    // trait, rather than a panic on hand-authored IR.
    for value in [json!(["declined"]), json!("declined"), json!([1, "x"])] {
        let module = module(
            vec![error_shape(
                "m#declined",
                vec![
                    trait_of("status", json!([402])),
                    trait_of("errorCode", value),
                ],
            )],
            vec![],
        );
        let errors = declared_errors(&op(vec![], vec!["m#declined"]), &module);
        assert_eq!(errors[0].code, None);
    }
}

#[test]
fn duplicate_status_and_code_is_rejected_at_generation() {
    let shapes = vec![
        coded_error_shape("m#a", 400, "code", "bad"),
        coded_error_shape("m#b", 400, "code", "bad"),
    ];
    let m = model_of(shapes, op(vec![], vec!["m#a", "m#b"]));
    let err = validate_error_codes(&m).unwrap_err();
    assert!(err.contains("m#a") && err.contains("m#b"), "{err}");
}

#[test]
fn same_status_different_paths_is_allowed_at_generation() {
    let shapes = vec![
        coded_error_shape("m#a", 400, "code", "bad"),
        coded_error_shape("m#b", 400, "error.type", "bad"),
    ];
    let m = model_of(shapes, op(vec![], vec!["m#a", "m#b"]));
    assert!(validate_error_codes(&m).is_ok());
}

/// The gate walks an entry's nested operations too, not just a module's loose
/// ones: a collision declared only on an entry operation must still be caught.
#[test]
fn duplicate_status_and_code_on_a_nested_entry_op_is_rejected_at_generation() {
    let mut nested_op = op(vec![], vec!["m#a", "m#b"]);
    nested_op.id = "m#client.do_thing".into();
    let entry = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![],
            operations: vec![nested_op],
        },
        traits: vec![],
    };
    let shapes = vec![
        coded_error_shape("m#a", 400, "code", "bad"),
        coded_error_shape("m#b", 400, "code", "bad"),
        entry,
    ];
    let m = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module(shapes, vec![])],
    };
    let err = validate_error_codes(&m).unwrap_err();
    assert!(err.contains("m#a") && err.contains("m#b"), "{err}");
}

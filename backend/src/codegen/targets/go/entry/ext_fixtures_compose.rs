//! A library that *composes* its own foreign handles, split out of
//! `ext_fixtures` to keep that file under the file-size ceiling; `use
//! super::*` reaches its builders (`field`, `go_extern`, `go_ext_lib`, ...)
//! and IR imports.

use super::*;

/// `new_primary` and `new_secondary` each construct a `resource` handle, and
/// `new_combined` receives both back as arguments to build a third.
/// `combined` implements the entry's own op; `injected` is the same handle
/// type, but supplied by the caller (`@arg`) instead of constructed,
/// covering the other place a handle-typed value crosses tono's own
/// generated boundary. This is the shape none of the other fixtures
/// exercise: every other handle here is only ever consumed by
/// tono-generated code, never passed from one foreign call into another.
pub fn composed_handles_module() -> Module {
    let value = structure("t#value", vec![member("data", string_t(), true)]);

    let resource_t = Tref::Ref {
        id: "compose#resource".into(),
        args: vec![],
    };
    let value_t = Tref::Ref {
        id: "t#value".into(),
        args: vec![],
    };

    let a_field = field("a", string_t(), vec![Source::Arg]);
    let b_field = field("b", string_t(), vec![Source::Arg]);
    let c_field = field("c", string_t(), vec![Source::Arg]);
    let d_field = field("d", string_t(), vec![Source::Arg]);
    let injected_field = field("injected", resource_t.clone(), vec![Source::Arg]);

    let primary_field = {
        let mut f = field("primary", resource_t.clone(), vec![]);
        f.call = Some(EntryCall {
            ns: "compose".into(),
            func: "new_primary".into(),
            args: vec![
                call_ref(&["a"]),
                call_ref(&["b"]),
                call_ref(&["c"]),
                call_ref(&["d"]),
            ],
        });
        f
    };
    let secondary_field = {
        let mut f = field("secondary", resource_t.clone(), vec![]);
        f.call = Some(EntryCall {
            ns: "compose".into(),
            func: "new_secondary".into(),
            args: vec![call_ref(&["b"]), call_ref(&["c"])],
        });
        f
    };
    let combined_field = {
        let mut f = field("combined", resource_t.clone(), vec![]);
        f.call = Some(EntryCall {
            ns: "compose".into(),
            func: "new_combined".into(),
            // Two already-constructed sibling handles, passed back in as
            // this call's own arguments.
            args: vec![
                call_ref(&["b"]),
                call_ref(&["primary"]),
                call_ref(&["secondary"]),
            ],
        });
        f
    };

    let combined_value_op = Shape {
        id: "t#client.combined_value".into(),
        kind: ShapeKind::Operation {
            input: None,
            input_name: None,
            output: Some(value_t.clone()),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["combined".into()],
                method: "get".into(),
                args: vec![],
            }),
        },
        traits: vec![],
    };
    let injected_value_op = Shape {
        id: "t#client.injected_value".into(),
        kind: ShapeKind::Operation {
            input: None,
            input_name: None,
            output: Some(value_t.clone()),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["injected".into()],
                method: "get".into(),
                args: vec![],
            }),
        },
        traits: vec![],
    };

    let entry = Shape {
        id: "t#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![
                a_field,
                b_field,
                c_field,
                d_field,
                injected_field,
                primary_field,
                secondary_field,
                combined_field,
            ],
            operations: vec![combined_value_op, injected_value_op],
        },
        traits: vec![],
    };

    let get_method = go_extern(
        "get",
        vec![],
        value_t.clone(),
        "Get",
        vec![],
        vec![YieldsPos {
            name: "v".into(),
            r#type: Some(Tref::Ref {
                id: "compose#go_value".into(),
                args: vec![],
            }),
            is_error: false,
        }],
        Some(ReturnsLit {
            r#type: value_t.clone(),
            fields: vec![ReturnsField {
                name: "data".into(),
                value: ReturnsValue::Field(vec!["v".into(), "Data".into()]),
            }],
        }),
        vec![],
    );

    let compose = go_ext_lib(
        "compose",
        "tono-ext-fixture/compose",
        vec![ForeignStruct {
            name: "go_value".into(),
            fields: vec![ForeignField {
                name: "Data".into(),
                r#type: string_t(),
            }],
        }],
        vec![OpaqueType {
            name: "resource".into(),
            interface: false,
            instance: None,
            methods: vec![get_method],
        }],
        vec![
            go_extern(
                "new_primary",
                vec![
                    ext_param("a", string_t()),
                    ext_param("b", string_t()),
                    ext_param("c", string_t()),
                    ext_param("d", string_t()),
                ],
                resource_t.clone(),
                "NewPrimary",
                vec![
                    CallArg::Param("a".into()),
                    CallArg::Param("b".into()),
                    CallArg::Param("c".into()),
                    CallArg::Param("d".into()),
                ],
                vec![],
                None,
                vec![],
            ),
            go_extern(
                "new_secondary",
                vec![ext_param("b", string_t()), ext_param("c", string_t())],
                resource_t.clone(),
                "NewSecondary",
                vec![CallArg::Param("b".into()), CallArg::Param("c".into())],
                vec![],
                None,
                vec![],
            ),
            go_extern(
                "new_combined",
                vec![
                    ext_param("b", string_t()),
                    ext_param("primary", resource_t.clone()),
                    ext_param("secondary", resource_t.clone()),
                ],
                resource_t,
                "NewCombined",
                vec![
                    CallArg::Param("b".into()),
                    CallArg::Param("primary".into()),
                    CallArg::Param("secondary".into()),
                ],
                vec![],
                None,
                vec![],
            ),
        ],
    );

    Module {
        name: "t".into(),
        shapes: vec![value, entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![compose],
        tests: vec![],
    }
}

/// [`composed_handles_module`], wrapped in a `Model`.
pub fn composed_handles_model() -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![composed_handles_module()],
    }
}

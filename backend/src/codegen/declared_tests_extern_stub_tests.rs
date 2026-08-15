//! The extern-stub coverage of the declared-test planner: fixtures for two
//! extern shapes (`companyconfig.load`, a free function; `companybus.Publisher.send`,
//! an opaque handle method) and the tests planning them against, both happy
//! paths and every rejection. Split out of `declared_tests_tests` to keep it
//! under the file-size ceiling; the shared fixtures (`construction`,
//! `test_with_externs`, `entry_tests`, ...) come from the parent module via
//! `super::*`.

use super::*;

/// `companyconfig.load` (a free extern fn) and `companybus.Publisher.send`
/// (an opaque handle method) -- the two extern shapes these fixtures exercise.
fn ext_libs() -> Vec<crate::ir::ExtLib> {
    vec![
        crate::ir::ExtLib {
            name: "companyconfig".into(),
            langs: vec![],
            structs: vec![],
            types: vec![],
            externs: vec![ExternDecl {
                name: "load".into(),
                params: vec![],
                r#return: Tref::Prim(Prim::String),
                langs: vec![],
            }],
        },
        crate::ir::ExtLib {
            name: "companybus".into(),
            langs: vec![],
            structs: vec![],
            types: vec![OpaqueType {
                name: "Publisher".into(),
                methods: vec![ExternDecl {
                    name: "send".into(),
                    params: vec![],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![],
                }],
            }],
            externs: vec![],
        },
    ]
}

/// [`module`], extended with `ext_libs`, a construction-time free-fn-sourced
/// field (`cfg`), a call-time foreign-handle field (`bus`), and a `publish`
/// op whose `impl` body reaches `bus.send`.
fn ext_module(tests: Vec<TestDecl>) -> Module {
    let mut m = module(tests);
    m.ext_libs = ext_libs();
    let ShapeKind::Entry { fields, operations } = &mut m.shapes[1].kind else {
        panic!("expected the entry shape");
    };
    fields.push(crate::ir::EntryField {
        name: "cfg".into(),
        target: Tref::Prim(Prim::String),
        sources: vec![],
        format: None,
        transforms: vec![],
        select: None,
        call: Some(crate::ir::EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![],
        }),
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    });
    fields.push(crate::ir::EntryField {
        name: "bus".into(),
        target: Tref::Ref {
            id: "companybus#Publisher".into(),
            args: vec![],
        },
        sources: vec![],
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    });
    operations.push(Shape {
        id: "notes#client.publish".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Prim(Prim::String)),
            output: Some(Tref::Prim(Prim::String)),
            errors: vec![Tref::Ref {
                id: "notes#overloaded".into(),
                args: vec![],
            }],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["bus".into()],
                method: "send".into(),
                args: vec![],
            }),
        },
        traits: vec![],
    });
    m
}

fn free_extern_stub(answers: Vec<StubAnswer>) -> ExternStub {
    ExternStub {
        binding: None,
        target: ExternStubTarget::Free {
            lib: "companyconfig".into(),
            fn_: "load".into(),
        },
        answers,
    }
}

fn method_extern_stub(answers: Vec<StubAnswer>) -> ExternStub {
    ExternStub {
        binding: None,
        target: ExternStubTarget::Method {
            lib: "companybus".into(),
            ty: "Publisher".into(),
            method: "send".into(),
        },
        answers,
    }
}

fn value_answer(v: serde_json::Value) -> StubAnswer {
    StubAnswer::Value { value: v }
}

#[test]
fn a_test_with_a_free_extern_stub_and_a_call_scoped_stub_plans() {
    let t = test_with_externs(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![free_extern_stub(vec![value_answer(serde_json::json!(
            "cfg"
        ))])],
        vec![call("fetch_note")],
        vec![],
    );
    let m = ext_module(vec![t]);
    let groups = entry_tests(&m).expect("plans clean");
    assert_eq!(groups[0].tests[0].extern_stubs.len(), 1);
}

#[test]
fn a_test_reaching_an_unstubbed_construction_time_extern_call_is_rejected() {
    let t = test_with_externs(vec![construction()], vec![], vec![], vec![], vec![]);
    let err = entry_tests(&ext_module(vec![t])).unwrap_err();
    assert!(
        err.contains("reaches 'companyconfig.load' during construction, which is not stubbed"),
        "unexpected error: {err}"
    );
}

#[test]
fn a_test_reaching_an_unstubbed_call_time_handle_method_is_rejected() {
    let t = test_with_externs(
        vec![construction()],
        vec![],
        vec![free_extern_stub(vec![value_answer(serde_json::json!(
            "cfg"
        ))])],
        vec![call("publish")],
        vec![],
    );
    let err = entry_tests(&ext_module(vec![t])).unwrap_err();
    assert!(
        err.contains("reaches 'companybus.Publisher.send' during the call, which is not stubbed"),
        "unexpected error: {err}"
    );
}

#[test]
fn a_handle_method_stub_may_answer_with_a_declared_error_shape() {
    let t = test_with_externs(
        vec![construction()],
        vec![],
        vec![
            free_extern_stub(vec![value_answer(serde_json::json!("cfg"))]),
            method_extern_stub(vec![StubAnswer::Error {
                error: AnswerError {
                    shape: "overloaded".into(),
                    data: serde_json::json!({"message": "shed"}),
                },
            }]),
        ],
        vec![call("publish")],
        vec![],
    );
    let m = ext_module(vec![t]);
    let groups = entry_tests(&m).expect("plans clean");
    assert_eq!(groups[0].tests[0].extern_stubs.len(), 2);
}

#[test]
fn a_free_extern_stub_answering_with_a_declared_error_is_rejected() {
    let t = test_with_externs(
        vec![construction()],
        vec![],
        vec![free_extern_stub(vec![StubAnswer::Error {
            error: AnswerError {
                shape: "overloaded".into(),
                data: serde_json::json!({}),
            },
        }])],
        vec![],
        vec![],
    );
    let err = entry_tests(&ext_module(vec![t])).unwrap_err();
    assert!(
        err.contains("is not a plain value"),
        "unexpected error: {err}"
    );
}

#[test]
fn an_extern_stub_naming_an_unknown_lib_is_rejected() {
    let mut stub = free_extern_stub(vec![value_answer(serde_json::json!("cfg"))]);
    stub.target = ExternStubTarget::Free {
        lib: "nope".into(),
        fn_: "load".into(),
    };
    let t = test_with_externs(vec![construction()], vec![], vec![stub], vec![], vec![]);
    let err = entry_tests(&ext_module(vec![t])).unwrap_err();
    assert!(
        err.contains("names no declared ext lib"),
        "unexpected error: {err}"
    );
}

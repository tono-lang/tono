use std::collections::BTreeMap;

use super::*;
use crate::ir::{
    AnswerError, Empty, ExtKind, Extension, HttpAnswer, Prim, ShapePattern, TaxonomyPattern,
    TemplatePart, Trait, Tref, WireBinding, WireValue,
};

fn op(id: &str, traits: Vec<Trait>, errors: Vec<Tref>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Prim(Prim::String)),
            output: Some(Tref::Prim(Prim::String)),
            errors,
            wire: None,
        },
        traits,
    }
}

fn http_op(id: &str) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Prim(Prim::String)),
            output: Some(Tref::Prim(Prim::String)),
            errors: vec![],
            wire: Some(Box::new(WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
                bindings: Default::default(),
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: None,
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
        },
        traits: vec![],
    }
}

fn error_shape() -> Shape {
    Shape {
        id: "notes#overloaded".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![
            Trait {
                id: "status".into(),
                value: serde_json::json!([529]),
            },
            Trait {
                id: "errorCode".into(),
                value: serde_json::json!(["code", "overloaded"]),
            },
        ],
    }
}

fn impl_ext(raw: bool) -> Extension {
    Extension {
        name: "save_note".into(),
        kind: ExtKind::Impl,
        signature: None,
        raw,
        bindings: BTreeMap::from([("go".to_string(), "ext/go/x.go#X".to_string())]),
        conformance: None,
    }
}

/// A module with one entry `client` holding an `@http` op (`fetch_note`) and a
/// bespoke op (`save_note`, declared error `overloaded`).
fn module(tests: Vec<TestDecl>) -> Module {
    Module {
        name: "notes".into(),
        shapes: vec![
            error_shape(),
            Shape {
                id: "notes#client".into(),
                kind: ShapeKind::Entry {
                    fields: vec![crate::ir::EntryField {
                        name: "api_key".into(),
                        target: Tref::Prim(Prim::String),
                        sources: vec![crate::ir::Source::Arg],
                        format: None,
                        transforms: vec![],
                        select: None,
                        binds: vec![],
                        constraints: vec![],
                        traits: vec![],
                    }],
                    operations: vec![
                        http_op("notes#client.fetch_note"),
                        op(
                            "notes#client.save_note",
                            vec![],
                            vec![Tref::Ref {
                                id: "notes#overloaded".into(),
                                args: vec![],
                            }],
                        ),
                    ],
                },
                traits: vec![],
            },
        ],
        operations: vec![],
        extensions: vec![impl_ext(false)],
        tests,
    }
}

fn construction() -> TestConstruction {
    TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([("api_key".to_string(), serde_json::json!("k"))]),
    }
}

fn call(op: &str) -> TestCall {
    TestCall {
        binding: "got".into(),
        client: "c".into(),
        op: op.into(),
        input: Some(serde_json::json!("x")),
    }
}

fn http_stub(answers: Vec<StubAnswer>) -> TestStub {
    TestStub {
        binding: Some("s".into()),
        client: "c".into(),
        op: "fetch_note".into(),
        dep: StubDep::Http,
        answers,
    }
}

fn http_answer(status: i64) -> StubAnswer {
    StubAnswer::Http(HttpAnswer {
        status,
        headers: BTreeMap::new(),
        body: "\"pong\"".into(),
    })
}

fn test(
    constructions: Vec<TestConstruction>,
    stubs: Vec<TestStub>,
    calls: Vec<TestCall>,
    expects: Vec<TestExpect>,
) -> TestDecl {
    TestDecl {
        name: "the case".into(),
        constructions,
        stubs,
        calls,
        expects,
    }
}

fn err_of(tests: Vec<TestDecl>) -> String {
    entry_tests(&module(tests)).unwrap_err()
}

#[test]
fn tests_group_per_entry_and_derive_hermetic_or_live() {
    let hermetic = test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("fetch_note")],
        vec![],
    );
    let live = test(
        vec![construction()],
        vec![],
        vec![call("fetch_note")],
        vec![],
    );
    let construction_only = test(vec![construction()], vec![], vec![], vec![]);
    let m = module(vec![hermetic, live, construction_only]);
    let groups = entry_tests(&m).expect("plans clean");
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].entry, "client");
    let flags: Vec<bool> = groups[0].tests.iter().map(|t| t.hermetic).collect();
    assert_eq!(flags, vec![true, false, true]);
    // The stubbed test resolved its call's operation shape.
    assert_eq!(
        groups[0].tests[0].op.map(|op| bare_op_name(&op.id)),
        Some("fetch_note")
    );
    assert_eq!(
        entries_with_tests(&m),
        std::collections::BTreeSet::from(["client".to_string()])
    );
}

#[test]
fn a_multi_call_flow_is_a_loud_phase_error() {
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("fetch_note"), call("save_note")],
        vec![],
    )]);
    assert!(
        err.contains("multi-call flows are not generated yet"),
        "{err}"
    );
    assert!(err.contains("the case"), "{err}");
}

#[test]
fn a_binding_reference_in_the_input_is_a_loud_phase_error() {
    let mut c = call("fetch_note");
    c.input = Some(serde_json::json!({"id": {"$ref": ["saved", "id"]}}));
    let err = err_of(vec![test(vec![construction()], vec![], vec![c], vec![])]);
    assert!(
        err.contains("multi-call flows are not generated yet"),
        "{err}"
    );
}

#[test]
fn the_call_must_name_an_operation_of_the_constructed_entry() {
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("archive_note")],
        vec![],
    )]);
    assert!(err.contains("not an operation"), "{err}");
}

#[test]
fn the_stub_dep_must_match_the_operation_binding() {
    // An http stub on the bespoke op is a category error.
    let mut stub = http_stub(vec![http_answer(200)]);
    stub.op = "save_note".into();
    let err = err_of(vec![test(
        vec![construction()],
        vec![stub],
        vec![call("save_note")],
        vec![],
    )]);
    assert!(err.contains("no @http binding"), "{err}");
    // An impl stub on the @http op is too.
    let stub = TestStub {
        binding: None,
        client: "c".into(),
        op: "fetch_note".into(),
        dep: StubDep::Impl,
        answers: vec![StubAnswer::Contract { contract: Empty {} }],
    };
    let err = err_of(vec![test(
        vec![construction()],
        vec![stub],
        vec![call("fetch_note")],
        vec![],
    )]);
    assert!(err.contains("bound to @http"), "{err}");
}

#[test]
fn an_impl_stub_needs_the_ext_impl_binding() {
    let mut m = module(vec![test(
        vec![construction()],
        vec![TestStub {
            binding: None,
            client: "c".into(),
            op: "save_note".into(),
            dep: StubDep::Impl,
            answers: vec![StubAnswer::Contract { contract: Empty {} }],
        }],
        vec![call("save_note")],
        vec![],
    )]);
    m.extensions.clear();
    let err = entry_tests(&m).unwrap_err();
    assert!(err.contains("no ext impl"), "{err}");
}

#[test]
fn a_stub_without_answers_or_off_the_called_op_is_rejected() {
    let err = err_of(vec![test(
        vec![construction()],
        vec![http_stub(vec![])],
        vec![call("fetch_note")],
        vec![],
    )]);
    assert!(err.contains("no answers"), "{err}");
    let err = err_of(vec![test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("save_note")],
        vec![],
    )]);
    assert!(
        err.contains("stubs 'fetch_note' but calls 'save_note'"),
        "{err}"
    );
    let err = err_of(vec![test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![],
        vec![],
    )]);
    assert!(err.contains("never calls"), "{err}");
}

#[test]
fn an_error_answer_must_name_a_declared_error() {
    let stub = TestStub {
        binding: None,
        client: "c".into(),
        op: "save_note".into(),
        dep: StubDep::Impl,
        answers: vec![StubAnswer::Error {
            error: AnswerError {
                shape: "unknown_error".into(),
                data: serde_json::json!({}),
            },
        }],
    };
    let err = err_of(vec![test(
        vec![construction()],
        vec![stub],
        vec![call("save_note")],
        vec![],
    )]);
    assert!(err.contains("not a declared error"), "{err}");
}

#[test]
fn a_value_naming_no_entry_field_is_rejected() {
    let mut c = construction();
    c.values.insert("token".to_string(), serde_json::json!("t"));
    let err = err_of(vec![test(
        vec![c],
        vec![],
        vec![call("fetch_note")],
        vec![],
    )]);
    assert!(err.contains("pins 'token'"), "{err}");
}

fn outcome(subject: &str, pattern: TestPattern) -> TestExpect {
    TestExpect::Outcome {
        subject: subject.into(),
        pattern,
    }
}

#[test]
fn outcome_expects_resolve_to_the_call_or_the_construction() {
    // Over the call's binding: fine.
    let ok = test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("fetch_note")],
        vec![outcome("got", TestPattern::Ok(Empty {}))],
    );
    assert!(entry_tests(&module(vec![ok])).is_ok());
    // Over a name that is neither: refused.
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("fetch_note")],
        vec![outcome("other", TestPattern::Ok(Empty {}))],
    )]);
    assert!(err.contains("not the call's binding"), "{err}");
    // A construction-only test expects over the construction, with ok or a
    // taxonomy category only.
    let cfg = TestPattern::Taxonomy(TaxonomyPattern {
        category: "config".into(),
        open: true,
        fields: BTreeMap::new(),
    });
    let ok = test(
        vec![construction()],
        vec![],
        vec![],
        vec![outcome("c", cfg)],
    );
    assert!(entry_tests(&module(vec![ok])).is_ok());
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![],
        vec![outcome(
            "c",
            TestPattern::Struct(ShapePattern {
                shape: "user".into(),
                open: true,
                fields: BTreeMap::new(),
            }),
        )],
    )]);
    assert!(err.contains("ok or an errors.*"), "{err}");
}

#[test]
fn an_error_pattern_must_name_a_declared_error_and_taxonomy_is_closed() {
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("save_note")],
        vec![outcome(
            "got",
            TestPattern::Error(ShapePattern {
                shape: "unknown_error".into(),
                open: false,
                fields: BTreeMap::new(),
            }),
        )],
    )]);
    assert!(err.contains("not a declared error"), "{err}");
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("save_note")],
        vec![outcome(
            "got",
            TestPattern::Taxonomy(TaxonomyPattern {
                category: "timeout".into(),
                open: true,
                fields: BTreeMap::new(),
            }),
        )],
    )]);
    assert!(err.contains("unknown error category"), "{err}");
    let err = err_of(vec![test(
        vec![construction()],
        vec![],
        vec![call("save_note")],
        vec![outcome(
            "got",
            TestPattern::Taxonomy(TaxonomyPattern {
                category: "api".into(),
                open: true,
                fields: BTreeMap::from([(
                    "path".to_string(),
                    FieldPattern::Pat(TestPattern::Eq(serde_json::json!("/x"))),
                )]),
            }),
        )],
    )]);
    assert!(err.contains("'errors.api' has no field 'path'"), "{err}");
}

#[test]
fn request_expects_resolve_to_the_http_stub_binding() {
    let open_request = RequestPattern {
        open: true,
        fields: BTreeMap::from([(
            "method".to_string(),
            FieldPattern::Pat(TestPattern::Eq(serde_json::json!("GET"))),
        )]),
        headers: None,
    };
    let ok = test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("fetch_note")],
        vec![TestExpect::Requests {
            subject: "s".into(),
            requests: vec![open_request.clone()],
        }],
    );
    assert!(entry_tests(&module(vec![ok])).is_ok());
    // The subject must be the stub's binding.
    let err = err_of(vec![test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("fetch_note")],
        vec![TestExpect::Requests {
            subject: "t".into(),
            requests: vec![open_request.clone()],
        }],
    )]);
    assert!(err.contains("not the stub's binding"), "{err}");
    // A closed pattern is a loud refusal, not a silently weaker check.
    let closed = RequestPattern {
        open: false,
        ..open_request
    };
    let err = err_of(vec![test(
        vec![construction()],
        vec![http_stub(vec![http_answer(200)])],
        vec![call("fetch_note")],
        vec![TestExpect::Requests {
            subject: "s".into(),
            requests: vec![closed],
        }],
    )]);
    assert!(err.contains("closed request patterns"), "{err}");
}

#[test]
fn the_committed_fixture_decodes_and_names_its_entry() {
    // The cross-language contract fixture is the arbiter of the wire shape. It
    // is a codec-stage IR (its @http op still carries the raw `http` trait, not
    // the resolved `wire` binding), so full planning is not run over it;
    // decoding and the seam-gating derivation must accept it as-is.
    let json = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../ir-schema/fixtures/declared_tests.json"
    ));
    let model = crate::ir::decode_model(json).expect("fixture decodes");
    assert_eq!(
        entries_with_tests(&model.modules[0]),
        std::collections::BTreeSet::from(["client".to_string()])
    );
}

#[test]
fn a_closed_all_eq_pattern_collapses_to_its_whole_wire_object() {
    let eq = |v: serde_json::Value| FieldPattern::Pat(TestPattern::Eq(v));
    let closed = ShapePattern {
        shape: "note".into(),
        open: false,
        fields: BTreeMap::from([
            ("id".to_string(), eq(serde_json::json!("n1"))),
            ("body".to_string(), eq(serde_json::json!("hello"))),
        ]),
    };
    assert_eq!(
        closed_eq_object(&closed),
        Some(serde_json::json!({"id": "n1", "body": "hello"}))
    );
    // A closed pattern with no fields pins the empty object.
    let empty = ShapePattern {
        shape: "note".into(),
        open: false,
        fields: BTreeMap::new(),
    };
    assert_eq!(closed_eq_object(&empty), Some(serde_json::json!({})));
    // An open pattern tolerates unmentioned fields, so it cannot collapse.
    let open = ShapePattern {
        open: true,
        ..closed.clone()
    };
    assert_eq!(closed_eq_object(&open), None);
    // A marker leaf (present/absent) is not an equality, so no collapse.
    let mut marked = closed;
    marked.fields.insert(
        "note".to_string(),
        FieldPattern::Absent { absent: Empty {} },
    );
    assert_eq!(closed_eq_object(&marked), None);
}

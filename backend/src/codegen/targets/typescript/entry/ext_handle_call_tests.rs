use super::super::test_prelude::*;
use super::*;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, EntryField, ExtLib, ExternDecl,
    ExternLang as IrExternLang, ForeignField, ForeignStruct, LangPath, OpImplCall, OpaqueType,
    Prim, ReturnsField, ReturnsLit, ReturnsValue, Select, SelectArm, Shape, ShapeKind, Source,
    Tref, YieldsPos,
};

use super::super::ext_fixtures::ef;

fn string_param(name: &str) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: Tref::Prim(Prim::String),
    }
}

fn string_field(name: &str) -> ForeignField {
    ForeignField {
        name: name.into(),
        r#type: Tref::Prim(Prim::String),
    }
}

/// The `bus` handle: `send` exercises the full `yields`/`returns`/`errors`
/// path (a sibling field and the op's own input parameter as arguments),
/// `ping` is a bare pass-through (no `yields`), `status` projects through a
/// `match`.
fn bus_lib() -> ExtLib {
    let status_t = Tref::Ref {
        id: "m#status".into(),
        args: vec![],
    };
    let send = super::super::ext_fixtures::send_method("m#ack", "m#raw_ack");
    let ping = ExternDecl {
        name: "ping".into(),
        params: vec![],
        r#return: Tref::Prim(Prim::String),
        langs: vec![IrExternLang {
            lang: "ts".into(),
            symbol: "ping".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            chain: None,
        }],
        r#async: vec!["ts".into()],
        errors: vec![],
    };
    let status = ExternDecl {
        name: "status".into(),
        params: vec![],
        r#return: status_t.clone(),
        langs: vec![IrExternLang {
            lang: "ts".into(),
            symbol: "status".into(),
            call_args: vec![],
            yields: vec![YieldsPos {
                name: "st".into(),
                r#type: Some(Tref::Ref {
                    id: "bus#raw_status".into(),
                    args: vec![],
                }),
                is_error: false,
                foreign: None,
            }],
            returns: Some(ReturnsLit {
                r#type: status_t,
                fields: vec![ReturnsField {
                    name: "label".into(),
                    value: ReturnsValue::Select(Select {
                        subject: vec!["st".into(), "code".into()],
                        subject_index: None,
                        arms: vec![
                            SelectArm {
                                pattern: Some(serde_json::json!("OK")),
                                value: ArmValue::Lit(serde_json::json!("healthy")),
                            },
                            SelectArm {
                                pattern: None,
                                value: ArmValue::Field(vec!["st".into(), "message".into()]),
                            },
                        ],
                    }),
                }],
            }),
            chain: None,
        }],
        r#async: vec!["ts".into()],
        errors: vec![],
    };
    // `tag` exercises `render_call_arg`'s `Lit`/`List`/`Ctor` branches
    // directly in the call template (no `Param` substitution involved);
    // `echo` exercises a bare (whole-value) reference to the op's own
    // input parameter, the single-segment form of a `Ref`.
    let mut ctor_fields = std::collections::BTreeMap::new();
    ctor_fields.insert("k".to_string(), CallArg::Lit(serde_json::json!("v")));
    let tag = ExternDecl {
        name: "tag".into(),
        params: vec![],
        r#return: Tref::Prim(Prim::String),
        langs: vec![IrExternLang {
            lang: "ts".into(),
            symbol: "tag".into(),
            call_args: vec![
                CallArg::Lit(serde_json::json!("v1")),
                CallArg::List(vec![
                    CallArg::Lit(serde_json::json!("a")),
                    CallArg::Lit(serde_json::json!("b")),
                ]),
                CallArg::Ctor(CallCtor {
                    name: "opts".into(),
                    fields: ctor_fields,
                    spelling: None,
                }),
            ],
            yields: vec![],
            returns: None,
            chain: None,
        }],
        r#async: vec!["ts".into()],
        errors: vec![],
    };
    let echo = ExternDecl {
        name: "echo".into(),
        params: vec![string_param("msg")],
        r#return: Tref::Prim(Prim::String),
        langs: vec![IrExternLang {
            lang: "ts".into(),
            symbol: "echo".into(),
            call_args: vec![CallArg::Param("msg".into())],
            yields: vec![],
            returns: None,
            chain: None,
        }],
        r#async: vec!["ts".into()],
        errors: vec![],
    };
    let connect = ExternDecl {
        name: "connect".into(),
        params: vec![],
        r#return: Tref::Ref {
            id: "bus#publisher".into(),
            args: vec![],
        },
        langs: vec![IrExternLang {
            lang: "ts".into(),
            symbol: "connect".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            chain: None,
        }],
        r#async: vec!["ts".into()],
        errors: vec![],
    };
    ExtLib {
        name: "bus".into(),
        langs: vec![LangPath {
            lang: "ts".into(),
            path: "@fixture/bus".into(),
        }],
        structs: vec![
            ForeignStruct {
                name: "raw_ack".into(),
                fields: vec![string_field("ok")],
                langs: vec![],
            },
            ForeignStruct {
                name: "raw_status".into(),
                fields: vec![string_field("code"), string_field("message")],
                langs: vec![],
            },
        ],
        types: vec![OpaqueType {
            name: "publisher".into(),
            langs: vec![crate::ir::ForeignLang {
                lang: "ts".into(),
                name: "Publisher".into(),
                fields: Default::default(),
            }],
            methods: vec![send, ping, status, tag, echo],
        }],
        externs: vec![connect],
    }
}

fn publish_input_shape() -> Shape {
    structure(
        "m#publish_input",
        vec![member("body", Tref::Prim(Prim::String), true)],
    )
}

fn ack_shape() -> Shape {
    structure("m#ack", vec![member("ok", Tref::Prim(Prim::String), true)])
}

fn status_shape() -> Shape {
    structure(
        "m#status",
        vec![member("label", Tref::Prim(Prim::String), true)],
    )
}

fn bus_field() -> EntryField {
    let mut f = ef(
        "bus",
        Tref::Ref {
            id: "bus#publisher".into(),
            args: vec![],
        },
        vec![Source::With],
        Some(EntryCall {
            ns: "bus".into(),
            func: "connect".into(),
            args: vec![],
        }),
    );
    f.sources = vec![Source::With];
    f
}

fn topic_field() -> EntryField {
    ef("topic", Tref::Prim(Prim::String), vec![Source::Arg], None)
}

fn publish_op() -> Shape {
    Shape {
        id: "m#client.publish".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#publish_input".into(),
                args: vec![],
            }),
            input_name: Some("msg".into()),
            output: Some(Tref::Ref {
                id: "m#ack".into(),
                args: vec![],
            }),
            output_nullable: false,
            errors: vec![],
            wire: None,
            impl_call: Some(super::super::ext_fixtures::send_op_impl_call()),
        },
        traits: vec![],
    }
}

/// An op with no declared input, implemented as a bare no-argument call
/// into the `bus` handle's own `method`: the shape `heartbeat_op`,
/// `status_op`, and `tag_op` all share, differing only in which method
/// they call and what it returns.
fn no_input_op(id: &str, method: &str, output: Tref) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input: None,
            input_name: None,
            output: Some(output),
            output_nullable: false,
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["bus".into()],
                method: method.into(),
                args: vec![],
            }),
        },
        traits: vec![],
    }
}

fn heartbeat_op() -> Shape {
    no_input_op("m#client.heartbeat", "ping", Tref::Prim(Prim::String))
}

fn status_op() -> Shape {
    no_input_op(
        "m#client.status",
        "status",
        Tref::Ref {
            id: "m#status".into(),
            args: vec![],
        },
    )
}

fn tag_op() -> Shape {
    no_input_op("m#client.tag", "tag", Tref::Prim(Prim::String))
}

fn echo_op() -> Shape {
    Shape {
        id: "m#client.echo".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#publish_input".into(),
                args: vec![],
            }),
            input_name: Some("msg".into()),
            output: Some(Tref::Prim(Prim::String)),
            output_nullable: false,
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["bus".into()],
                method: "echo".into(),
                args: vec![CallArg::Ref(vec!["msg".into()])],
            }),
        },
        traits: vec![],
    }
}

fn module_with_ops(ops: Vec<Shape>) -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![
            publish_input_shape(),
            ack_shape(),
            status_shape(),
            super::super::ext_fixtures::overloaded_shape("m"),
            Shape {
                id: "m#client".into(),
                kind: ShapeKind::Entry {
                    fields: vec![topic_field(), bus_field()],
                    operations: ops,
                },
                traits: vec![],
            },
        ],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![bus_lib()],
    }
}

fn rendered_text(module: &Module) -> String {
    let emission = emit(module, &ts_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &TsRules)
}

#[test]
fn a_handle_method_call_awaits_the_receiver_typed_by_its_own_generated_interface() {
    let out = rendered_text(&module_with_ops(vec![publish_op()]));
    // The call itself lives in the op's module-scoped seam (read off its
    // `Settings` parameter), and the method goes through that seam, so a
    // declared test can stub the handle method by swapping one binding.
    assert!(
        out.contains("let publishHandleCall: (s: Settings, input: PublishInput) => Promise<Ack> = async (s, input) => {"),
        "{out}"
    );
    assert!(
        out.contains("const raw = await s.bus.send(s.topic, input.body);"),
        "{out}"
    );
    assert!(
        out.contains("return await publishHandleCall(this.settings, input);"),
        "{out}"
    );
    // Without a declared test there is no swapper to export.
    assert!(!out.contains("swapPublishHandleCallForTest"), "{out}");
    assert!(out.contains("export interface PublisherHandle {"), "{out}");
    assert!(
        out.contains("send(topic: string, body: string): Promise<RawAck>;"),
        "{out}"
    );
}

#[test]
fn a_yields_projection_reads_the_foreign_verbatim_member_and_casts_to_the_logical_type() {
    let out = rendered_text(&module_with_ops(vec![publish_op()]));
    assert!(out.contains("return { ok: raw.ok };"), "{out}");
}

#[test]
fn a_declared_sentinel_throws_the_generated_typed_error() {
    let out = rendered_text(&module_with_ops(vec![publish_op()]));
    assert!(
        out.contains("if (e instanceof BusyError) { throw new OverloadedError(e); }"),
        "{out}"
    );
    assert!(
        out.contains("export class OverloadedError extends TonoError"),
        "{out}"
    );
}

#[test]
fn an_unmapped_failure_falls_back_to_contract_error_naming_the_call() {
    let out = rendered_text(&module_with_ops(vec![publish_op()]));
    assert!(
        out.contains("throw new ContractError(\"bus.send\", e);"),
        "{out}"
    );
}

#[test]
fn a_method_with_no_yields_narrows_the_honestly_unknown_raw_result_to_the_op_s_own_output() {
    let out = rendered_text(&module_with_ops(vec![heartbeat_op()]));
    assert!(out.contains("const raw = await s.bus.ping();"), "{out}");
    assert!(out.contains("ping(): Promise<unknown>;"), "{out}");
    assert!(out.contains("return raw as string;"), "{out}");
}

#[test]
fn a_match_inside_returns_lowers_to_an_immediately_invoked_switch() {
    let out = rendered_text(&module_with_ops(vec![status_op()]));
    assert!(out.contains("switch (raw.code) {"), "{out}");
    assert!(out.contains("case \"OK\": return \"healthy\";"), "{out}");
    assert!(out.contains("default: return raw.message;"), "{out}");
}

#[test]
fn a_call_template_s_literal_list_and_ctor_arguments_render_verbatim() {
    let out = rendered_text(&module_with_ops(vec![tag_op()]));
    assert!(
        out.contains("await s.bus.tag(\"v1\", [\"a\", \"b\"], { k: \"v\" });"),
        "{out}"
    );
}

#[test]
fn a_bare_reference_to_the_op_s_own_input_reads_the_whole_parameter() {
    let out = rendered_text(&module_with_ops(vec![echo_op()]));
    assert!(out.contains("await s.bus.echo(input);"), "{out}");
}

/// A declared test whose call is an `impl .bus.send(..)` op, hermetic
/// through its extern stubs alone (no call-scoped stub): the generated test
/// swaps the op's own seam for the canned logical answer, and the handle
/// field's constructor stub installs a fake handle whose methods only fail
/// loudly, so neither the real constructor nor the real method is reached.
fn stubbed_publish_module(answer: crate::ir::StubAnswer) -> Module {
    use crate::ir::{ExternStub, ExternStubTarget, TestCall, TestConstruction, TestDecl};
    use std::collections::BTreeMap;

    let mut module = module_with_ops(vec![publish_op()]);
    module.tests = vec![TestDecl {
        name: "publishes through the stub".into(),
        constructions: vec![TestConstruction {
            binding: "c".into(),
            entry: "client".into(),
            values: BTreeMap::from([("topic".to_string(), serde_json::json!("news"))]),
        }],
        stubs: vec![],
        extern_stubs: vec![
            ExternStub {
                binding: None,
                target: ExternStubTarget::Free {
                    lib: "bus".into(),
                    fn_: "connect".into(),
                },
                answers: vec![crate::ir::StubAnswer::Value {
                    value: serde_json::json!({}),
                }],
            },
            ExternStub {
                binding: None,
                target: ExternStubTarget::Method {
                    lib: "bus".into(),
                    ty: "publisher".into(),
                    method: "send".into(),
                },
                answers: vec![answer],
            },
        ],
        calls: vec![TestCall {
            binding: "got".into(),
            client: "c".into(),
            op: "publish".into(),
            input: Some(serde_json::json!({"body": "hi"})),
        }],
        expects: vec![],
    }];
    module
}

fn hermetic_test_text(module: &Module) -> String {
    let files = super::super::vector_tests::test_files(module, &ts_casing());
    let hermetic = files
        .iter()
        .find(|f| f.group.tests_of() == Some(("client", false)))
        .expect("a hermetic test file");
    rendered(&hermetic.file.decls, &TsRules)
}

#[test]
fn a_handle_method_stub_on_the_called_op_swaps_the_op_s_seam_in_the_generated_hermetic_test() {
    let module = stubbed_publish_module(crate::ir::StubAnswer::Value {
        value: serde_json::json!({"ok": "yes"}),
    });
    let out = hermetic_test_text(&module);
    assert!(
        out.contains("swapPublishHandleCallForTest(async () => decodeAck({\"ok\":\"yes\"}))"),
        "the op's seam must be swapped for the decoded logical answer: {out}"
    );
    // The handle field's own stub installs a fake satisfying the handle
    // interface, never a decoder the module does not have.
    assert!(out.contains("swapBusExtForTest(async () => ({"), "{out}");
    assert!(
        out.contains("bus.publisher.send: no stub for this call in test"),
        "{out}"
    );
    assert!(!out.contains("decodePublisher"), "{out}");
    // Hermetic on the extern stubs alone: bare construction, no transport.
    assert!(out.contains("const c = await Client.create("), "{out}");
    assert!(!out.contains("forTest"), "{out}");
    // And the client exports the swapper for the tested entry.
    let client = rendered_text(&module);
    assert!(
        client.contains("export function swapPublishHandleCallForTest("),
        "{client}"
    );
}

#[test]
fn a_handle_method_stub_answering_a_declared_sentinel_error_throws_its_typed_class() {
    let module = stubbed_publish_module(crate::ir::StubAnswer::Error {
        error: crate::ir::AnswerError {
            shape: "overloaded".into(),
            data: serde_json::json!({"message": "busy"}),
        },
    });
    let out = hermetic_test_text(&module);
    assert!(
        out.contains("throw new OverloadedError({\"message\":\"busy\"});"),
        "{out}"
    );
}

#[test]
fn a_handle_method_stub_answering_a_shape_no_ts_sentinel_maps_throws_a_plain_error() {
    let module = stubbed_publish_module(crate::ir::StubAnswer::Error {
        error: crate::ir::AnswerError {
            shape: "throttled".into(),
            data: serde_json::json!({}),
        },
    });
    let out = hermetic_test_text(&module);
    assert!(
        out.contains("throw new Error(\"simulated throttled\");"),
        "{out}"
    );
    assert!(!out.contains("ThrottledError"), "{out}");
}

/// A handle method whose `yields` list projects nothing narrows the raw
/// result to the op's own output, the way a method with no `yields` does.
#[test]
fn a_signature_yields_list_narrows_the_raw_result_to_the_op_s_own_output() {
    let mut module = module_with_ops(vec![status_op()]);
    let status = module.ext_libs[0].types[0]
        .methods
        .iter_mut()
        .find(|m| m.name == "status")
        .unwrap();
    status.langs[0].returns = None;
    let out = rendered_text(&module);
    assert!(!out.contains("switch (raw.code)"), "{out}");
    assert!(out.contains("return raw as Status;"), "{out}");
}

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
    let send = super::super::ext_fixtures::send_method("m#ack", "bus#raw_ack");
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
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
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
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
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
                }),
            ],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
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
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
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
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
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
            },
            ForeignStruct {
                name: "raw_status".into(),
                fields: vec![string_field("code"), string_field("message")],
            },
        ],
        types: vec![OpaqueType {
            name: "publisher".into(),
            instance: None,
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
fn a_handle_method_call_awaits_the_cast_receiver_with_its_declared_arguments() {
    let out = rendered_text(&module_with_ops(vec![publish_op()]));
    assert!(
        out.contains(
            "const raw = await ((this.settings.bus) as any).send(this.settings.topic, input.body);"
        ),
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
        out.contains("case \"BUSY\": throw new OverloadedError(e);"),
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
fn a_method_with_no_yields_returns_the_raw_result_directly() {
    let out = rendered_text(&module_with_ops(vec![heartbeat_op()]));
    assert!(
        out.contains("const raw = await ((this.settings.bus) as any).ping();"),
        "{out}"
    );
    assert!(out.contains("return raw;"), "{out}");
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
        out.contains(
            "await ((this.settings.bus) as any).tag(\"v1\", [\"a\", \"b\"], { k: \"v\" });"
        ),
        "{out}"
    );
}

#[test]
fn a_bare_reference_to_the_op_s_own_input_reads_the_whole_parameter() {
    let out = rendered_text(&module_with_ops(vec![echo_op()]));
    assert!(
        out.contains("await ((this.settings.bus) as any).echo(input);"),
        "{out}"
    );
}

//! Worked `ext`/`extern` fixtures shared between this module's own unit
//! tests and the external `tests/ts_ext_roundtrip.rs` compiled-vector check
//! (the latter can only reach `pub` crate items, the same reason Go's own
//! `targets::go::entry::ext_fixtures` exists), so a spec exercised by both
//! is built once instead of drifting apart as two copies.

use crate::ir::{
    CallArg, CallCtor, EntryCall, EntryField, ErrorBinding, ExternDecl, ExternLang, ExternParam,
    OpImplCall, Prim, ReturnsField, ReturnsLit, ReturnsValue, Source, Tref, YieldsPos,
};
use std::collections::BTreeMap;

/// An entry field, built from exactly the parts a worked `ext` example
/// varies: its declared type, how it is sourced, and (for a constructed
/// field) the call that produces it. Shared by this module's own unit tests
/// and the external `ts_ext_roundtrip.rs` compiled-vector check.
pub fn ef(name: &str, target: Tref, sources: Vec<Source>, call: Option<EntryCall>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// A handle method `send(topic, body) -> ack`, `ts` binding: a full
/// `yields`/`returns` projection plus one declared sentinel, the shape a
/// realistic handle-method call exercises end to end. `ack_id` and
/// `raw_ack_id` are the caller's own module-qualified ids for the logical
/// ack shape and the foreign struct `yields` binds, so the same builder fits
/// a spec under any module name.
pub fn send_method(ack_id: &str, raw_ack_id: &str) -> ExternDecl {
    let ack_t = Tref::Ref {
        id: ack_id.into(),
        args: vec![],
    };
    ExternDecl {
        name: "send".into(),
        params: vec![
            ExternParam {
                name: "topic".into(),
                r#type: Tref::Prim(Prim::String),
            },
            ExternParam {
                name: "body".into(),
                r#type: Tref::Prim(Prim::String),
            },
        ],
        r#return: ack_t.clone(),
        langs: vec![ExternLang {
            lang: "ts".into(),
            symbol: "send".into(),
            call_args: vec![
                CallArg::Param("topic".into()),
                CallArg::Param("body".into()),
            ],
            yields: vec![YieldsPos {
                name: "ack".into(),
                r#type: Some(Tref::Ref {
                    id: raw_ack_id.into(),
                    args: vec![],
                }),
                is_error: false,
            }],
            returns: Some(ReturnsLit {
                r#type: ack_t,
                fields: vec![ReturnsField {
                    name: "ok".into(),
                    value: ReturnsValue::Field(vec!["ack".into(), "ok".into()]),
                }],
            }),
            errors: vec![ErrorBinding {
                sentinel: "BUSY".into(),
                r#type: "overloaded".into(),
            }],
            sync: false,
            infallible: false,
            ctx: false,
        }],
    }
}

/// A handle constructor `connect(endpoint, token) -> publisher`, `ts`
/// binding: a bare construction (no `yields`), the shape a real injectable
/// handle field's own `field.call` exercises. `publisher_id` is the
/// caller's own module-qualified id for the opaque handle type.
pub fn connect_publisher_extern(publisher_id: &str) -> ExternDecl {
    ExternDecl {
        name: "connect".into(),
        params: vec![
            ExternParam {
                name: "endpoint".into(),
                r#type: Tref::Prim(Prim::String),
            },
            ExternParam {
                name: "token".into(),
                r#type: Tref::Prim(Prim::String),
            },
        ],
        r#return: Tref::Ref {
            id: publisher_id.into(),
            args: vec![],
        },
        langs: vec![ExternLang {
            lang: "ts".into(),
            symbol: "connect".into(),
            call_args: vec![
                CallArg::Param("endpoint".into()),
                CallArg::Param("token".into()),
            ],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        }],
    }
}

/// An op's own body: a call into the `bus` field's `send` method, reading
/// a sibling `topic` field and the op's own declared input parameter's
/// `body` member as its two arguments -- the shape [`send_method`]'s own
/// `params`/`call_args` expect. Shared by this module's own unit tests and
/// the external `ts_ext_roundtrip.rs` compiled-vector check, both of which
/// pair a `send_method` handle with an op implemented exactly this way.
pub fn send_op_impl_call() -> OpImplCall {
    OpImplCall {
        recv: vec!["bus".into()],
        method: "send".into(),
        args: vec![
            CallArg::Ref(vec!["topic".into()]),
            CallArg::Ref(vec!["msg".into(), "body".into()]),
        ],
    }
}

/// The appendix worked example's own `companyconfig.load(service, region) -> app_config`,
/// `ts` binding: a `Ctor` argument (the foreign `ts_opts` struct, its two
/// fields substituted positionally from the op's own params), a
/// `yields`/`returns` pair projecting the foreign `ts_config` struct's
/// verbatim field names onto the declared `app_config` shape, and one
/// declared sentinel. `app_config_id` is the caller's own module-qualified
/// id for that shape. Shared by this module's own unit tests and the
/// external `ts_ext_roundtrip.rs` compiled-vector check, both of which build
/// the appendix example under a different module name.
pub fn load_config_extern(app_config_id: &str) -> ExternDecl {
    let mut load_ctor_fields = BTreeMap::new();
    load_ctor_fields.insert("region".to_string(), CallArg::Param("region".into()));
    load_ctor_fields.insert("service".to_string(), CallArg::Param("service".into()));
    let app_config_t = Tref::Ref {
        id: app_config_id.into(),
        args: vec![],
    };
    ExternDecl {
        name: "load".into(),
        params: vec![
            ExternParam {
                name: "service".into(),
                r#type: Tref::Prim(Prim::String),
            },
            ExternParam {
                name: "region".into(),
                r#type: Tref::Prim(Prim::String),
            },
        ],
        r#return: app_config_t.clone(),
        langs: vec![ExternLang {
            lang: "ts".into(),
            symbol: "load".into(),
            call_args: vec![CallArg::Ctor(CallCtor {
                name: "ts_opts".into(),
                fields: load_ctor_fields,
            })],
            yields: vec![YieldsPos {
                name: "cfg".into(),
                r#type: Some(Tref::Ref {
                    id: "companyconfig#ts_config".into(),
                    args: vec![],
                }),
                is_error: false,
            }],
            returns: Some(ReturnsLit {
                r#type: app_config_t,
                fields: vec![
                    ReturnsField {
                        name: "endpoint".into(),
                        value: ReturnsValue::Field(vec!["cfg".into(), "host".into()]),
                    },
                    ReturnsField {
                        name: "token".into(),
                        value: ReturnsValue::Field(vec!["cfg".into(), "token".into()]),
                    },
                ],
            }),
            errors: vec![ErrorBinding {
                sentinel: "BUSY".into(),
                r#type: "overloaded".into(),
            }],
            sync: false,
            infallible: false,
            ctx: false,
        }],
    }
}

/// The appendix worked example's own `config` and `bus` entry fields: `config` a plain
/// call into [`load_config_extern`], `bus` a `@with`-fallback call into
/// [`connect_publisher_extern`] reading `config`'s own resolved members.
/// `app_config_id`/`publisher_id` are the caller's own module-qualified ids,
/// matching whatever [`load_config_extern`]/[`connect_publisher_extern`] the
/// caller declared its `ext` libs with.
pub fn appendix_config_and_bus_fields(
    app_config_id: &str,
    publisher_id: &str,
) -> (EntryField, EntryField) {
    let config = ef(
        "config",
        Tref::Ref {
            id: app_config_id.into(),
            args: vec![],
        },
        vec![],
        Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![
                CallArg::Ref(vec!["service".into()]),
                CallArg::Ref(vec!["region".into()]),
            ],
        }),
    );
    let mut bus = ef(
        "bus",
        Tref::Ref {
            id: publisher_id.into(),
            args: vec![],
        },
        vec![Source::With],
        Some(EntryCall {
            ns: "companybus".into(),
            func: "connect".into(),
            args: vec![
                CallArg::Ref(vec!["config".into(), "endpoint".into()]),
                CallArg::Ref(vec!["config".into(), "token".into()]),
            ],
        }),
    );
    bus.sources = vec![Source::With];
    (config, bus)
}

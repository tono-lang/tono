//! Worked `ext`/`extern` fixtures shared between this module's own unit
//! tests and the external `tests/ts_ext_roundtrip.rs` compiled-vector check
//! (the latter can only reach `pub` crate items, the same reason Go's own
//! `targets::go::entry::ext_fixtures` exists), so a spec exercised by both
//! is built once instead of drifting apart as two copies.

use crate::ir::{
    EntryCall, EntryField, ErrorBinding, ExternDecl, ExternLang, ExternParam, Prim, ReturnsField,
    ReturnsLit, ReturnsValue, Source, Tref, YieldsPos,
};

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
                crate::ir::CallArg::Param("topic".into()),
                crate::ir::CallArg::Param("body".into()),
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

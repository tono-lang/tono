//! The boundary check helpers: construction-time `ConfigError`s, the
//! resolved-value casts the frozen `values` map needs, and the presence guard
//! around a frozen entry. The duration parser and env reader an entry's
//! resolution logic needs live in the SDK-root `duration`/`env` groups
//! ([`super::shared`]); a bytes-typed field reuses the `bytes` group's own
//! `base64_bytes::decode`, the same helper a wire struct field routes through
//! (see `HelperSet` in `rust/codecs.rs`) — an entry needs no copy of its own.

use super::*;

/// A construction-time failure returning the SDK's dedicated `ConfigError`
/// category as a full `return Err(..);` statement. `message_expr` is an
/// already-built Rust string expression (a `format!(..)` call or a literal).
/// Every bad env value, malformed blob, absent member, or unmatched select is
/// a config problem, discriminable via `TonoError::Config` from a transport,
/// validation, or declared error.
pub(super) fn config_error(message_expr: &str) -> String {
    format!("return Err(TonoError::Config(ConfigError {{ message: {message_expr} }}));")
}

/// The settings-field read for a value path's own field, honoring its
/// `@rename(rust)` override; a member path tail is plain (unrenamed).
pub(super) fn access(vp: &crate::codegen::entries::ValuePath<'_>, config: &CasingConfig) -> String {
    let head = field_snake_ren(
        &vp.field.name,
        rename_of(&vp.field.traits, LANG).as_deref(),
        config,
    );
    match &vp.member {
        None => head,
        Some(member) => format!("{}.{}", head, field_snake(member, config)),
    }
}

/// Whether a value path freezes into the runtime values, and if so, the
/// (already-Rust) expression reading it off `s`. A named reference freezes
/// when it resolves to a scalar (an enum is a branded string on the wire),
/// which is what `scalar_ref` says; a map/list never freezes as a single
/// value.
pub(super) fn value_expr(
    vp: &crate::codegen::entries::ValuePath<'_>,
    config: &CasingConfig,
    scalar_ref: bool,
) -> Option<String> {
    if matches!(vp.target, Tref::Ref { .. }) && !scalar_ref {
        return None;
    }
    if matches!(vp.target, Tref::Map(_, _) | Tref::List(_)) {
        return None;
    }
    Some(format!("s.{}", access(vp, config)))
}

/// The conversion of a resolved field expression into a `serde_json::Value`
/// for the frozen `values` map. `serde_json::Value::from` covers every
/// numeric width and `bool` directly; string-shaped values clone into an
/// owned `String` first (the expression reads through a borrow of `s`).
/// `Duration` is handled by the caller (it freezes as milliseconds, not its
/// wire string), never reaching here.
pub(super) fn value_cast(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(
            Prim::Bool
            | Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64
            | Prim::Float,
        ) => format!("serde_json::Value::from({expr})"),
        Tref::Prim(Prim::String | Prim::Uuid) => format!("serde_json::Value::from({expr}.clone())"),
        Tref::Prim(Prim::Timestamp | Prim::Date) => {
            format!("serde_json::Value::from({expr}.0.clone())")
        }
        Tref::Prim(Prim::Bytes) => {
            // No struct field in this module is guaranteed to have routed
            // bytes through the base64 helper, so an entry field of type
            // `bytes` consumed by a descriptor ref position (a rare shape:
            // headers/timeouts/retries are otherwise string/numeric) freezes
            // through a lossy UTF-8 read rather than pulling in a codec this
            // file does not otherwise need.
            format!("serde_json::Value::from(String::from_utf8_lossy(&{expr}).into_owned())")
        }
        Tref::Prim(Prim::Duration) => unreachable!("duration freezes as ms; handled by the caller"),
        // Only an enum-typed ref reaches here (value_expr's scalar_ref
        // gate); Display gives its wire spelling directly.
        Tref::Ref { .. } => format!("serde_json::Value::from({expr}.to_string())"),
        _ => format!("{expr}.clone()"),
    }
}

/// The presence condition guarding a value entry, or `None` when the value is
/// always frozen (the decision lives in
/// [`crate::codegen::entries::needs_presence_guard`]; this only spells the
/// comparison). String-shaped targets compare against their empty/zero form;
/// `PartialEq` on the branded well-known types and the open enums makes this
/// comparison valid.
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
    module: &Module,
    config: &CasingConfig,
) -> Option<String> {
    crate::codegen::entries::needs_presence_guard(entry, vp)
        .then(|| format!("{expr} != {}", zero_value(vp.target, module, config)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::rust::rust_casing;
    use crate::codegen::test_support::{bare_entry_field, push_entry_field};
    use crate::ir::Source;

    fn m() -> Module {
        Module {
            name: "m".into(),
            shapes: vec![crate::ir::Shape {
                id: "m#client".into(),
                kind: ShapeKind::Entry {
                    fields: vec![],
                    operations: vec![],
                },
                traits: vec![],
            }],
            operations: vec![],
            extensions: vec![],
        }
    }

    #[test]
    fn config_error_wraps_the_message_expression() {
        assert_eq!(
            config_error("\"bad\".to_string()"),
            "return Err(TonoError::Config(ConfigError { message: \"bad\".to_string() }));"
        );
    }

    #[test]
    fn value_cast_of_bytes_reads_lossily_since_no_base64_helper_is_guaranteed() {
        assert_eq!(
            value_cast(&Tref::Prim(Prim::Bytes), "s.receipt"),
            "serde_json::Value::from(String::from_utf8_lossy(&s.receipt).into_owned())"
        );
    }

    #[test]
    fn value_cast_falls_back_to_a_clone_for_a_container_type() {
        assert_eq!(
            value_cast(&Tref::List(Box::new(Tref::Prim(Prim::String))), "s.tags"),
            "s.tags.clone()"
        );
    }

    #[test]
    fn a_duration_default_pulls_in_the_parser_helper() {
        let mut module = m();
        push_entry_field(
            &mut module,
            bare_entry_field(
                "wait",
                Tref::Prim(Prim::Duration),
                vec![Source::Default(serde_json::json!("5s"))],
            ),
        );
        let out = entry_text(&module, &rust_casing());
        assert!(out.contains("use crate::duration::{parse_duration_ms};"));
    }
}

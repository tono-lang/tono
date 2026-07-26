//! The boundary check helpers (the narrow-integer ranges the env parses
//! enforce, and the container/element checks that keep a whole-JSON field as
//! strict as Go's typed unmarshal) plus the frozen-value spellings the
//! constructor hands the runtime.

use super::*;

pub(super) fn access(vp: &crate::codegen::entries::ValuePath<'_>, config: &CasingConfig) -> String {
    match &vp.member {
        None => field_camel(&vp.field.name, config),
        Some(member) => format!(
            "{}.{}",
            field_camel(&vp.field.name, config),
            field_camel(member, config)
        ),
    }
}

/// Whether a value path freezes into the runtime values. A named reference
/// freezes when it resolves to a scalar (an enum is a branded string), which
/// is what `scalar_ref` says.
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

/// The conversion into the runtime's value positions: bigints narrow to
/// numbers (the descriptor's numeric refs), everything else passes as is.
pub(super) fn value_cast(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("Number({expr})"),
        _ => expr.to_string(),
    }
}

/// The presence condition guarding a value entry, or `None` when the value is
/// always frozen. The guard reads the resolved value itself, not the declared
/// chain's why-reason: client_init runs over the Settings before this point
/// (bespoke wins), so a hook-filled field must freeze like a declared one. A
/// non-string value freezes unconditionally (its zero means the same thing to
/// the runtime's value positions as its absence).
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
) -> Option<String> {
    if entry.is_guaranteed(vp.field) && vp.member.is_none() {
        return None;
    }
    if !string_like(vp.target) {
        return None;
    }
    Some(format!("{expr} !== {}", cast_string(vp.target, "\"\"")))
}

/// The inclusive range of a narrow integer, for the boundary check that
/// mirrors Go's strconv bit-size enforcement.
pub(super) fn int_bounds(p: &Prim) -> (&'static str, &'static str) {
    match p {
        Prim::I8 => ("-128", "127"),
        Prim::I16 => ("-32768", "32767"),
        Prim::I32 => ("-2147483648", "2147483647"),
        Prim::U8 => ("0", "255"),
        Prim::U16 => ("0", "65535"),
        _ => ("0", "4294967295"),
    }
}

pub(super) fn prim_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "value",
    }
}

/// The wire spelling a scalar element must have inside a whole-JSON value:
/// the `typeof` it decodes from, a description for the error, and whether it
/// must also be an integer. 64-bit integers ride the wire as strings.
fn scalar_expectation(t: &Tref) -> Option<(&'static str, &'static str, bool)> {
    match t {
        Tref::Prim(
            Prim::String
            | Prim::Uuid
            | Prim::Timestamp
            | Prim::Date
            | Prim::Duration
            | Prim::Bytes
            | Prim::I64
            | Prim::U64,
        ) => Some(("string", "a string", false)),
        Tref::Prim(Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32) => {
            Some(("number", "an integer", true))
        }
        Tref::Prim(Prim::Float) => Some(("number", "a number", false)),
        Tref::Prim(Prim::Bool) => Some(("boolean", "a boolean", false)),
        _ => None,
    }
}

/// The container and element checks for a whole-JSON env value, emitted between
/// the parse and the wire decode. Go's typed unmarshal rejects a mistyped
/// element, so this boundary must too or the same env value would construct in
/// one target and not the other. Relative to column zero; the caller nests it.
pub(super) fn json_shape_checks(t: &Tref, label: &str) -> String {
    let int_guard = |int: bool| {
        if int {
            " || !Number.isInteger(val)"
        } else {
            ""
        }
    };
    match t {
        Tref::Map(_, v) => {
            let mut out = format!(
                "if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n  throw new Error(`${{{label}}}: expected an object`);\n}}"
            );
            if let Some((ts_typeof, describe, int)) = scalar_expectation(v) {
                out.push_str(&format!(
                    "\nfor (const [key, val] of Object.entries(parsed)) {{\n  if (typeof val !== {ts_typeof:?}{ic}) {{\n    throw new Error(`${{{label}}}: field ${{key}} must be {describe}`);\n  }}\n}}",
                    ic = int_guard(int),
                ));
            }
            out
        }
        Tref::List(v) => {
            let mut out = format!(
                "if (!Array.isArray(parsed)) {{\n  throw new Error(`${{{label}}}: expected an array`);\n}}"
            );
            if let Some((ts_typeof, describe, int)) = scalar_expectation(v) {
                out.push_str(&format!(
                    "\n(parsed as unknown[]).forEach((val, i) => {{\n  if (typeof val !== {ts_typeof:?}{ic}) {{\n    throw new Error(`${{{label}}}: element ${{i}} must be {describe}`);\n  }}\n}});",
                    ic = int_guard(int),
                ));
            }
            out
        }
        // A union or other named shape decodes through its own codec, which
        // rejects malformed input itself.
        _ => String::new(),
    }
}

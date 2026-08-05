//! The boundary check helpers (the narrow-integer ranges the env parses
//! enforce, and the container/element checks that keep a whole-JSON field as
//! strict as Go's typed unmarshal) plus the frozen-value spellings the
//! constructor hands the runtime.

use super::*;

/// A construction-time failure throwing the SDK's dedicated ConfigError
/// category with `message` (a string or template-literal expression). Every
/// bad env value, malformed blob, absent member, or unmatched select is a
/// config problem, discriminable via `instanceof` from a transport,
/// validation, or declared error. The class file imports this category when
/// the resolver emits one.
pub(super) fn config_error(message: &str) -> String {
    format!(
        "throw new {config}({message});",
        config = error_names().config
    )
}

/// The settings-field read for a value path's own field, honoring its
/// `@rename(ts)` override. The casing transform itself has no TypeScript-
/// specific step, so it lives once as
/// [`crate::codegen::entries::value_path_access`], shared with Rust (the
/// other target whose settings receiver is a bare `s`).
pub(super) fn access(vp: &crate::codegen::entries::ValuePath<'_>, config: &CasingConfig) -> String {
    crate::codegen::entries::value_path_access(vp, config, LANG)
}

/// `<root>.<path>`, honoring `@rename(ts)` on the leading segment only (a
/// config/struct member's own name is never renamed, only an entry field's
/// is) — the rule the constructor's own resolver ([`super::resolve::Resolver::path_expr`])
/// and the transport's settings reads share, since both walk the same
/// field-path shape rooted at a different identifier (`s` inside the
/// constructor, `this.settings` from an operation method).
pub(super) fn field_path_expr(
    entry: &EntryModel<'_>,
    config: &CasingConfig,
    path: &[String],
    root: &str,
) -> String {
    let mut out = root.to_string();
    for (i, seg) in path.iter().enumerate() {
        out.push('.');
        if i == 0 {
            out.push_str(&field_camel_ren(
                seg,
                entry.field_rename(seg, LANG).as_deref(),
                config,
            ));
        } else {
            out.push_str(&field_camel(seg, config));
        }
    }
    out
}

/// A private, synthesized field name for a `@timeout` field path's converted
/// millisecond value: the same per-segment casing `field_path_expr` applies,
/// joined without dots (a tail segment capitalized, since this identifier is
/// synthesized rather than public) and suffixed `Ms`. Collision-free within
/// the class since it derives from the field's own (unique) path.
pub(super) fn timeout_field_name(
    entry: &EntryModel<'_>,
    config: &CasingConfig,
    path: &[String],
) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        if i == 0 {
            out.push_str(&field_camel_ren(
                seg,
                entry.field_rename(seg, LANG).as_deref(),
                config,
            ));
        } else {
            let tail = field_camel(seg, config);
            let mut chars = tail.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out.push_str("Ms");
    out
}

/// The presence condition guarding a value entry, or `None` when the value is
/// always frozen. The decision (a guaranteed non-member field is always present;
/// a non-string value freezes unconditionally) is shared; this target spells
/// only the comparison.
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
) -> Option<String> {
    crate::codegen::entries::needs_presence_guard(entry, vp)
        .then(|| format!("{expr} !== {}", cast_string(vp.target, "\"\"")))
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
                "if (typeof parsed !== \"object\" || parsed === null || Array.isArray(parsed)) {{\n  {}\n}}",
                config_error(&format!("`${{{label}}}: expected an object`")),
            );
            if let Some((ts_typeof, describe, int)) = scalar_expectation(v) {
                let fail = config_error(&format!(
                    "`${{{label}}}: field ${{key}} must be {describe}`"
                ));
                out.push_str(&format!(
                    "\nfor (const [key, val] of Object.entries(parsed)) {{\n  if (typeof val !== {ts_typeof:?}{ic}) {{\n    {fail}\n  }}\n}}",
                    ic = int_guard(int),
                ));
            }
            out
        }
        Tref::List(v) => {
            let mut out = format!(
                "if (!Array.isArray(parsed)) {{\n  {}\n}}",
                config_error(&format!("`${{{label}}}: expected an array`")),
            );
            if let Some((ts_typeof, describe, int)) = scalar_expectation(v) {
                let fail = config_error(&format!(
                    "`${{{label}}}: element ${{i}} must be {describe}`"
                ));
                out.push_str(&format!(
                    "\n(parsed as unknown[]).forEach((val, i) => {{\n  if (typeof val !== {ts_typeof:?}{ic}) {{\n    {fail}\n  }}\n}});",
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

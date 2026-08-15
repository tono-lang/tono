//! The `@env` leaf spellings for the Rust resolution plan: parsing a raw env
//! string into a field's declared type, and the small type-name tables that
//! parse uses. Split out of `resolve.rs` to stay within the file-size gate;
//! these are the least coupled leaves (only `field.target`/`EnvName`), so
//! moving them frees the most room with the least churn.

use super::resolve::Resolver;
use super::*;

/// An expression casting a Rust `String` into the field's target type. Only
/// valid for the string-shaped targets a `@format`/`@str::*` pipeline can
/// produce.
pub(super) fn cast_string(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        Tref::Prim(Prim::Timestamp) => format!("Timestamp({expr})"),
        Tref::Prim(Prim::Date) => format!("LocalDate({expr})"),
        Tref::Prim(Prim::Duration) => format!("Duration({expr})"),
        // An open enum accepts any wire value; a dynamically derived string
        // (not a compile-time literal, so the known-variant lookup
        // `literal_enum` uses is unavailable here) always resolves through
        // the Unknown catch-all, which serializes identically to a named
        // variant carrying the same wire spelling.
        Tref::Ref { id, .. } => format!("{}::Unknown({expr})", type_ident_from_id(id)),
        _ => expr.to_string(),
    }
}

pub(super) fn prim_rust_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "i64",
    }
}

pub(super) fn prim_label(p: &Prim) -> &'static str {
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

/// The statements parsing a raw env string `v` into `dest`, by the field's
/// declared type; a parse failure fails construction naming the variable
/// (`label_expr`) and the type. Relative to column zero.
pub(super) fn env_parse(
    r: &mut Resolver<'_, '_>,
    field: &EntryField,
    dest: &str,
    label_expr: &str,
) -> String {
    let t = &field.target;
    match t {
        Tref::Prim(Prim::Bool) => {
            let fail = checks::config_error(&format!(
                "format!(\"{{}}: invalid bool {{:?}} (want true/false/1/0)\", {label_expr}, v)"
            ));
            format!(
                "match v.as_str() {{\n    \"true\" | \"1\" => {{ {dest} = true; }}\n    \"false\" | \"0\" => {{ {dest} = false; }}\n    _ => {{ {fail} }}\n}}"
            )
        }
        Tref::Prim(
            p @ (Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64),
        ) => {
            let ty = prim_rust_name(p);
            let fail = checks::config_error(&format!(
                "format!(\"{{}}: invalid {prim} {{:?}}\", {label_expr}, v)",
                prim = prim_label(p),
            ));
            format!("match v.parse::<{ty}>() {{\n    Ok(n) => {{ {dest} = n; }}\n    Err(_) => {{ {fail} }}\n}}")
        }
        Tref::Prim(Prim::Float) => {
            let fail = checks::config_error(&format!(
                "format!(\"{{}}: invalid float {{:?}}\", {label_expr}, v)"
            ));
            // Decimal notation only: bare parse::<f64> also accepts
            // "inf"/"nan" spellings the Go/TypeScript boundary rejects.
            format!(
                "if !v.chars().all(|c| c.is_ascii_digit() || \"+-.eE\".contains(c)) {{\n    {fail}\n}}\nmatch v.parse::<f64>() {{\n    Ok(n) if n.is_finite() => {{ {dest} = n; }}\n    _ => {{ {fail} }}\n}}"
            )
        }
        Tref::Prim(Prim::Bytes) => {
            r.refs.push(Symbol::imported(
                "base64_bytes",
                crate::codegen::group::Group::root("bytes").path(),
                "base64_bytes",
            ));
            let fail = checks::config_error(&format!(
                "format!(\"{{}}: invalid base64 {{:?}}\", {label_expr}, v)"
            ));
            format!(
                "match {}::decode(&v) {{\n    Ok(b) => {{ {dest} = b; }}\n    Err(_) => {{ {fail} }}\n}}",
                shared_slot("base64_bytes")
            )
        }
        Tref::Prim(Prim::Duration) => {
            r.refs.push(shared_symbol("parse_duration_ms"));
            let fail = checks::config_error(&format!(
                "format!(\"{{}}: invalid duration {{:?}}\", {label_expr}, v)"
            ));
            format!(
                "if {}(&v).is_err() {{\n    {fail}\n}}\n{dest} = Duration(v.clone());",
                shared_slot("parse_duration_ms")
            )
        }
        _ => format!("{dest} = {};", cast_string(t, "v")),
    }
}

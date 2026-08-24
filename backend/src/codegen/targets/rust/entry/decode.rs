//! Decoding an operation's success payload into its declared output type.
//!
//! A structured output decodes leniently on what the contract promises:
//! required members must be present (`null`/absent is absence) and the shape
//! must parse; declared constraints are NOT enforced on the response (only on
//! what the client sends), and unknown fields are tolerated (the struct's own
//! derived `Deserialize` already does this) so a server adding a field does
//! not break the client. `rustfmt` (run as a normal step after codegen, like
//! every other target's output) owns final indentation, so this only needs
//! to be structurally valid, not perfectly laid out.
//!
//! `DecodeError.path` is produced by the top-level member probe below, not by
//! a recursive path-tracking decoder: a missing required member is named
//! (`$.field`), but a violation nested inside that member is not. This
//! mirrors Go/TS, which build `path` by hand at the same top level; adding
//! `serde_path_to_error` here would buy Rust alone a fidelity none of the
//! other targets have, at the cost of a dependency in every generated SDK.
//!
//! The required-member probe is a function of the output *type*, not of the
//! operation: two operations returning the same shape share one
//! `decode_<type>` (see [`output_decode_decl`]), so the probe is declared
//! once per type instead of once per call site.

use crate::codegen::conventions::{type_ident_from_id, wire_key};
use crate::codegen::tree::Decl;
use crate::ir::{Module, Prim, Shape, ShapeKind, Tref};

use super::rust_type;

/// The tail of a client method: an expression of type `Result<Output,
/// TonoError>` decoding `body_expr` (already the `&str` success body) into
/// the declared output, ready to sit as the last (semicolon-less) line of the
/// `Success` match arm. A nullable (`T?`) output treats an empty or JSON-null
/// success body as the declared absence (`Ok(None)`) before any typed decode
/// runs; a present body decodes exactly like the non-nullable form, wrapped
/// in `Some`.
pub(super) fn success_block(
    output: Option<&Tref>,
    nullable: bool,
    module: &Module,
    body_expr: &str,
) -> String {
    let Some(out) = output else {
        return "Ok(())".to_string();
    };
    if nullable {
        let decoded = decode_expr(out, module, "body");
        // A single-expression decode binds bare (a redundant block would trip
        // the generated SDK's unused_braces lint); the multi-statement i64
        // form keeps its block.
        let bind = if decoded.contains('\n') {
            format!("let decoded = {{\n{decoded}\n    }};")
        } else {
            format!("let decoded = {decoded};")
        };
        return format!(
            "let body = {body_expr};\n\
             let trimmed = body.trim();\n\
             if trimmed.is_empty() || trimmed == \"null\" {{\n\
             \x20   Ok(None)\n\
             }} else {{\n\
             \x20   {bind}\n\
             \x20   decoded.map(Some)\n\
             }}"
        );
    }
    match out {
        Tref::Ref { id, .. } => {
            let ty = type_ident_from_id(id);
            let out_shape = module.shapes.iter().find(|s| s.id == *id);
            if out_shape.is_some_and(shape_has_required) {
                // The probe lives once per type (see `output_decode_decl`);
                // the call site is just the call.
                format!("{}({body_expr})", output_decode_fn_name(&ty))
            } else {
                format!(
                    "let body = {body_expr};\n{}",
                    decode_expr(out, module, "body")
                )
            }
        }
        _ => format!(
            "let body = {body_expr};\n{}",
            decode_expr(out, module, "body")
        ),
    }
}

/// The `Result<T, TonoError>` expression decoding the already-bound `&str`
/// named by `body_ref` into `t`, shared by the nullable and plain forms of
/// [`success_block`].
fn decode_expr(t: &Tref, module: &Module, body_ref: &str) -> String {
    match t {
        Tref::Ref { id, .. } => {
            let ty = type_ident_from_id(id);
            let out_shape = module.shapes.iter().find(|s| s.id == *id);
            if out_shape.is_some_and(shape_has_required) {
                format!("{}({body_ref})", output_decode_fn_name(&ty))
            } else {
                let fail = format!(
                    "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: {body_ref}.to_string() }})"
                );
                format!("serde_json::from_str::<{ty}>({body_ref}).map_err(|_| {fail})")
            }
        }
        // A bare 64-bit integer output rides the wire as a JSON string; every
        // other bare primitive (and any container/enum, which decode through
        // their own derived/hand-written `Deserialize`) parses directly.
        Tref::Prim(Prim::I64 | Prim::U64) => {
            let ty = rust_type(t);
            let fail = format!(
                "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: {body_ref}.to_string() }})"
            );
            format!(
                "let wire: String = serde_json::from_str({body_ref}).map_err(|_| {fail})?;\nwire.parse::<{ty}>().map_err(|_| {fail})",
            )
        }
        _ => {
            let ty = rust_type(t);
            let fail = format!(
                "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: {body_ref}.to_string() }})"
            );
            format!("serde_json::from_str::<{ty}>({body_ref}).map_err(|_| {fail})")
        }
    }
}

fn shape_has_required(shape: &Shape) -> bool {
    matches!(&shape.kind, ShapeKind::Structure { members, .. } if members.iter().any(|m| m.required))
}

fn output_decode_fn_name(ty: &str) -> String {
    format!("decode_{}", super::snake(ty))
}

fn output_required_fields_const(ty: &str) -> String {
    format!("{}_REQUIRED_FIELDS", super::snake(ty).to_uppercase())
}

/// The per-type boundary parse a structured output with required members
/// shares across every operation that returns it: unmarshal into a probe
/// value, walk the required-member table, then the typed deserialize.
/// `None` for a shape with no required member (nothing to probe, nothing to
/// extract).
pub(super) fn output_decode_decl(shape: &Shape) -> Option<Decl> {
    let ShapeKind::Structure { members, .. } = &shape.kind else {
        return None;
    };
    let required: Vec<String> = members
        .iter()
        .filter(|m| m.required)
        .map(wire_key)
        .collect();
    if required.is_empty() {
        return None;
    }
    let ty = type_ident_from_id(&shape.id);
    let const_name = output_required_fields_const(&ty);
    let fn_name = output_decode_fn_name(&ty);
    let table = required
        .iter()
        .map(|f| format!("{f:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let fail_root = format!(
        "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: body.to_string() }})"
    );
    let text = format!(
        "/// The members {ty}'s declared shape requires present.\n\
         const {const_name}: &[&str] = &[{table}];\n\n\
         /// Parses a {ty} from its wire JSON: every required member must be present\n\
         /// (a null value counts as absent) before the typed decode runs.\n\
         pub(crate) fn {fn_name}(body: &str) -> Result<{ty}, TonoError> {{\n\
         \x20   let probe: serde_json::Value = serde_json::from_str(body).map_err(|_| {fail_root})?;\n\
         \x20   for field in {const_name} {{\n\
         \x20       if probe.get(field).map(|v| v.is_null()).unwrap_or(true) {{\n\
         \x20           return Err(TonoError::Decode(DecodeError {{ path: format!(\"$.{{field}}\"), expected: {ty:?}.to_string(), raw: body.to_string() }}));\n\
         \x20       }}\n\
         \x20   }}\n\
         \x20   serde_json::from_str::<{ty}>(body).map_err(|_| {fail_root})\n\
         }}",
    );
    Some(Decl::raw(text))
}

#[cfg(test)]
#[path = "decode_tests.rs"]
mod tests;

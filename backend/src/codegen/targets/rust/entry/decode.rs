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

use crate::codegen::conventions::{type_ident_from_id, wire_key};
use crate::ir::{Module, Prim, ShapeKind, Tref};

use super::rust_type;

/// The tail of a client method: an expression of type `Result<Output,
/// TonoError>` decoding `body_expr` (already the `&str` success body) into
/// the declared output, ready to sit as the last (semicolon-less) line of the
/// `Success` match arm.
pub(super) fn success_block(output: Option<&Tref>, module: &Module, body_expr: &str) -> String {
    match output {
        None => "Ok(())".to_string(),
        Some(Tref::Ref { id, .. }) => {
            let ty = type_ident_from_id(id);
            let out_shape = module.shapes.iter().find(|s| s.id == *id);
            let mut probe = String::new();
            if let Some(shape) = out_shape {
                if let ShapeKind::Structure { members, .. } = &shape.kind {
                    for m in members.iter().filter(|m| m.required) {
                        let name = wire_key(m);
                        probe.push_str(&format!(
                            "if probe.get({name:?}).map(|v| v.is_null()).unwrap_or(true) {{\n    return Err(TonoError::Decode(DecodeError {{ path: \"$.{name}\".to_string(), expected: {ty:?}.to_string(), raw: body.to_string() }}));\n}}\n",
                        ));
                    }
                }
            }
            let fail = format!(
                "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: body.to_string() }})"
            );
            if probe.is_empty() {
                format!("let body = {body_expr};\nserde_json::from_str::<{ty}>(body).map_err(|_| {fail})")
            } else {
                format!(
                    "let body = {body_expr};\nlet probe: serde_json::Value = serde_json::from_str(body).map_err(|_| {fail})?;\n{probe}serde_json::from_str::<{ty}>(body).map_err(|_| {fail})",
                )
            }
        }
        // A bare 64-bit integer output rides the wire as a JSON string; every
        // other bare primitive (and any container/enum, which decode through
        // their own derived/hand-written `Deserialize`) parses directly.
        Some(t @ Tref::Prim(Prim::I64 | Prim::U64)) => {
            let ty = rust_type(t);
            let fail = format!(
                "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: body.to_string() }})"
            );
            format!(
                "let body = {body_expr};\nlet wire: String = serde_json::from_str(body).map_err(|_| {fail})?;\nwire.parse::<{ty}>().map_err(|_| {fail})",
            )
        }
        Some(t) => {
            let ty = rust_type(t);
            let fail = format!(
                "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: {ty:?}.to_string(), raw: body.to_string() }})"
            );
            format!(
                "let body = {body_expr};\nserde_json::from_str::<{ty}>(body).map_err(|_| {fail})"
            )
        }
    }
}

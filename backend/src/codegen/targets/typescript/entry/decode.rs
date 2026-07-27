//! Decoding an operation's success payload into its declared output type.
//!
//! Shared by the two ways an operation gets a body: a protocol response through
//! the HTTP runtime, and a raw-form bespoke implementation returning an outcome.
//! Both name the payload `outcome.body` and both carry the same thing (the JSON
//! text of the declared output), so both decode it the same way.

use crate::codegen::conventions::{type_ident_from_id, wire_key};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::ir::{Module, ShapeKind, Tref};

use super::module_symbol;

/// The tail of a client method: decode `outcome.body` into `output` and return
/// it, or throw the mapped decode failure. `throw` routes an error expression
/// through the bound `on_error` hook when the module has one; `refs` collects
/// the imports the emitted code needs.
///
/// A structured output decodes leniently on what the contract promises:
/// required members must be present (undefined/null is absence) and the shape
/// must parse. Declared constraints are NOT enforced on the response (only on
/// what the client sends), and unknown fields are tolerated so a server adding a
/// field or loosening a bound does not break the client.
pub(super) fn success_block(
    output: Option<&Tref>,
    module: &Module,
    ret: &str,
    throw: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let en = error_names();
    match output {
        Some(Tref::Ref { id, .. }) => {
            refs.push(module_symbol(&en.decode, module));
            let out_name = type_ident_from_id(id);
            let t = throw(format!(
                "new {}(\"$\", \"{out_name}\", outcome.body)",
                en.decode
            ));
            let out_shape = module.shapes.iter().find(|s| s.id == *id);
            let mut required = String::new();
            if let Some(shape) = out_shape {
                if let ShapeKind::Structure { members, .. } = &shape.kind {
                    for m in members.iter().filter(|m| m.required) {
                        let name = wire_key(m);
                        // A missing required member points at that member (`$.tags`),
                        // not the whole body, so the caller sees which field the
                        // implementation omitted.
                        let miss = throw(format!(
                            "new {}(\"$.{name}\", \"{out_name}\", outcome.body)",
                            en.decode
                        ));
                        required.push_str(&format!(
                            "      if (!({name:?} in raw) || raw[{name:?}] === null) {{\n        {miss}\n      }}\n",
                        ));
                    }
                }
            }
            if required.is_empty() {
                format!(
                    "    try {{\n      return decode{out_name}(JSON.parse(outcome.body));\n    }} catch {{\n      {t}\n    }}",
                )
            } else {
                format!(
                    "    let raw: any;\n    try {{\n      raw = JSON.parse(outcome.body);\n    }} catch {{\n      {t}\n    }}\n\
                     \x20   if (typeof raw !== \"object\" || raw === null || Array.isArray(raw)) {{\n      {t}\n    }}\n\
                     {required}\
                     \x20   let out: {out_name};\n    try {{\n      out = decode{out_name}(raw);\n    }} catch {{\n      {t}\n    }}\n\
                     \x20   return out;",
                )
            }
        }
        Some(t) => {
            // A 64-bit integer (or a container holding one) rides the wire
            // as strings: the parsed body runs through the same decode the
            // codecs use so the method returns bigints, not raw JSON shapes.
            let decode = crate::codegen::targets::typescript::codecs::decode_expr(
                "JSON.parse(outcome.body)",
                t,
            );
            if decode == "JSON.parse(outcome.body)" {
                format!("    return {decode} as {ret};")
            } else {
                refs.push(module_symbol(&en.decode, module));
                format!(
                    "    try {{\n      return {decode} as {ret};\n    }} catch {{\n      {t}\n    }}",
                    t = throw(format!("new {}(\"$\", {ret:?}, outcome.body)", en.decode)),
                )
            }
        }
        None => "    return;".to_string(),
    }
}

//! Decoding an operation's success payload into its declared output type.
//!
//! Shared by the two ways an operation gets a body: a protocol response through
//! the HTTP runtime, and a raw-form bespoke implementation returning an outcome.
//! Both name the payload `outcome.body` and both carry the same thing (the JSON
//! text of the declared output), so both decode it the same way.
//!
//! The required-member probe is a function of the output *type*, not of the
//! operation: two operations returning the same shape share one
//! `parse<Type>` (see [`output_decode_decl`]), so the probe is declared once
//! per type instead of once per call site.

use crate::codegen::conventions::{type_ident_from_id, wire_key};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape, ShapeKind, Tref};

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
            let out_shape = module.shapes.iter().find(|s| s.id == *id);
            if out_shape.is_some_and(shape_has_required) {
                // The probe lives once per type (see `output_decode_decl`);
                // the call site only routes the caught path into its own
                // error boundary, naming which member (or the whole body)
                // came back wrong.
                refs.push(module_symbol(&format!("parse{out_name}"), module));
                let fail = throw(format!(
                    "new {}(path as string, \"{out_name}\", outcome.body)",
                    en.decode
                ));
                format!(
                    "    try {{\n      return parse{out_name}(outcome.body);\n    }} catch (path) {{\n      {fail}\n    }}",
                )
            } else {
                let t = throw(format!(
                    "new {}(\"$\", \"{out_name}\", outcome.body)",
                    en.decode
                ));
                format!(
                    "    try {{\n      return decode{out_name}(JSON.parse(outcome.body));\n    }} catch {{\n      {t}\n    }}",
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

fn shape_has_required(shape: &Shape) -> bool {
    matches!(&shape.kind, ShapeKind::Structure { members, .. } if members.iter().any(|m| m.required))
}

/// The per-type boundary parse a structured output with required members
/// shares across every operation that returns it: parse the JSON, walk the
/// required-member table (a `decode<Type>` alone is lenient, per the wire
/// codec's own contract), then the typed decode. Throws the failed path
/// ("$" for the whole body, "$.field" for a missing member) as a plain
/// string, for the caller to build its own `DecodeError`. `None` for a shape
/// with no required member (nothing to probe, nothing to extract).
pub(super) fn output_decode_decl(shape: &Shape, module: &Module) -> Option<Decl> {
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
    let fields_const = output_required_fields_const(&ty);
    let fn_name = output_decode_fn_name(&ty);
    let table = required
        .iter()
        .map(|f| format!("{f:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "/** The members {ty}'s declared shape requires present. */\n\
         const {fields_const} = [{table}] as const;\n\n\
         /**\n\
         \x20* Parses a {ty} from its wire JSON: every required member must be\n\
         \x20* present (a null value counts as absent) before the typed decode\n\
         \x20* runs. Throws the failed path (`$` for the whole body, `$.field` for\n\
         \x20* a missing member) as a plain string, for the caller to build its\n\
         \x20* own DecodeError.\n\
         \x20*/\n\
         function {fn_name}(raw: string): {ty} {{\n\
         \x20 let obj: any;\n\
         \x20 try {{\n\
         \x20   obj = JSON.parse(raw);\n\
         \x20 }} catch {{\n\
         \x20   throw \"$\";\n\
         \x20 }}\n\
         \x20 if (typeof obj !== \"object\" || obj === null || Array.isArray(obj)) {{\n\
         \x20   throw \"$\";\n\
         \x20 }}\n\
         \x20 for (const field of {fields_const}) {{\n\
         \x20   if (!(field in obj) || obj[field] === null) {{\n\
         \x20     throw \"$.\" + field;\n\
         \x20   }}\n\
         \x20 }}\n\
         \x20 return decode{ty}(obj);\n\
         }}",
    );
    Some(Decl::raw_with(
        text,
        vec![module_symbol(&format!("decode{ty}"), module)],
    ))
}

fn output_decode_fn_name(ty: &str) -> String {
    format!("parse{ty}")
}

fn output_required_fields_const(ty: &str) -> String {
    let mut chars = ty.chars();
    let lower_first: String = match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    };
    format!("{lower_first}RequiredFields")
}

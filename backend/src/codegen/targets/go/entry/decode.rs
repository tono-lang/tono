//! Decoding an operation's success payload into its declared output type.
//!
//! Shared by the two ways an operation gets a body: a protocol response through
//! the HTTP runtime, and a raw-form bespoke implementation returning an outcome.
//! Both carry the same thing (the JSON encoding of the declared output), so both
//! decode it the same way; only the Go expression naming the payload differs,
//! which is what [`Payload`] parameterizes.
//!
//! The required-member probe is a function of the output *type*, not of the
//! operation: two operations returning the same shape share one
//! `Decode<Type>` (see [`output_decode_decl`]), so the probe is declared once
//! per type instead of once per call site.

use crate::codegen::conventions::{type_ident_from_id, wire_key};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Module, Prim, Shape, ShapeKind, Tref};

use super::{go_type_label, import};

/// How the caller names the success payload, in the two spellings Go needs: as
/// text (for the `Raw:`/`Body:` fields the taxonomy carries) and as bytes (for
/// `json.Unmarshal`). The HTTP runtime hands over a string, a bespoke outcome
/// hands over bytes, so each supplies its own conversion once.
pub(super) struct Payload<'a> {
    pub text: &'a str,
    pub bytes: &'a str,
}

/// The tail of a client method: decode the payload into `output` and return it,
/// or return the mapped decode failure. `fail` turns an error expression into
/// the caller's own `return` statement; `refs` collects the imports the
/// emitted code needs.
///
/// A structured output decodes leniently on what the contract promises:
/// required members must be present (a zero value is not absence) and the shape
/// must parse. Declared constraints are NOT enforced on the response (only on
/// what the client sends), and unknown fields are tolerated so a server adding a
/// field or loosening a bound does not break the client.
pub(super) fn success_block(
    output: Option<&Tref>,
    module: &Module,
    payload: &Payload<'_>,
    fail: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let en = error_names();
    let (text, bytes) = (payload.text, payload.bytes);
    match output {
        // A 64-bit integer rides the wire as a string, so the success body is
        // a JSON string decoded and parsed, not a bare number.
        Some(t @ Tref::Prim(p @ (Prim::I64 | Prim::U64))) => {
            refs.push(import("json", "encoding/json"));
            refs.push(import("strconv", "strconv"));
            let fail_decode = fail(format!(
                "&{decode}{{Path: \"$\", Expected: {expected:?}, Raw: {text}}}",
                decode = en.decode,
                expected = go_type_label(t),
            ));
            let parse = if matches!(p, Prim::U64) {
                "strconv.ParseUint"
            } else {
                "strconv.ParseInt"
            };
            format!(
                "\tvar wire string\n\
                 \tif err := json.Unmarshal({bytes}, &wire); err != nil {{\n\
                 \t\treturn zero, {fail_decode}\n\t}}\n\
                 \tout, err := {parse}(wire, 10, 64)\n\
                 \tif err != nil {{\n\
                 \t\treturn zero, {fail_decode}\n\t}}\n\
                 \treturn out, nil"
            )
        }
        Some(t) => {
            let ty = go_type_label(t);
            let out_shape = match t {
                Tref::Ref { id, .. } => module.shapes.iter().find(|s| s.id == *id),
                _ => None,
            };
            if out_shape.is_some_and(shape_has_required) {
                // The probe lives once per type (see `output_decode_decl`, in
                // the codec file, which imports `encoding/json` itself); the
                // call site here only routes the failure through the
                // operation's own error boundary, naming which member (or
                // the whole body) came back wrong, and never touches `json`
                // directly.
                let fail_decode = fail(format!(
                    "&{decode}{{Path: path, Expected: {ty:?}, Raw: {text}}}",
                    decode = en.decode,
                ));
                format!(
                    "\tout, path, ok := {decode_fn}({bytes})\n\
                     \tif !ok {{\n\t\treturn zero, {fail_decode}\n\t}}\n\
                     \treturn out, nil",
                    decode_fn = output_decode_fn_name(&ty),
                )
            } else {
                refs.push(import("json", "encoding/json"));
                let fail_decode = fail(format!(
                    "&{decode}{{Path: \"$\", Expected: {ty:?}, Raw: {text}}}",
                    decode = en.decode,
                ));
                format!(
                    "\tvar out {ty}\n\
                     \tif err := json.Unmarshal({bytes}, &out); err != nil {{\n\
                     \t\treturn zero, {fail_decode}\n\t}}\n\
                     \treturn out, nil",
                )
            }
        }
        None => "\treturn nil".to_string(),
    }
}

fn shape_has_required(shape: &Shape) -> bool {
    matches!(&shape.kind, ShapeKind::Structure { members, .. } if members.iter().any(|m| m.required))
}

fn output_decode_fn_name(ty: &str) -> String {
    format!("Decode{ty}")
}

fn output_required_fields_var(ty: &str) -> String {
    let mut chars = ty.chars();
    let lower_first: String = match chars.next() {
        Some(c) => c.to_lowercase().chain(chars).collect(),
        None => String::new(),
    };
    format!("{lower_first}RequiredFields")
}

/// The per-type boundary parse a structured output with required members
/// shares across every operation that returns it: unmarshal into a probe map,
/// walk the required-member table, then the typed unmarshal. `None` for a
/// shape with no required member (nothing to probe, nothing to extract).
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
    let fields_var = output_required_fields_var(&ty);
    let fn_name = output_decode_fn_name(&ty);
    let table = required
        .iter()
        .map(|f| format!("{f:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "// {fields_var} names the members {ty}'s declared shape requires present.\n\
         var {fields_var} = []string{{{table}}}\n\n\
         // {fn_name} parses a {ty} from its wire JSON: every required member must be\n\
         // present (a null value counts as absent) before the typed decode runs. The\n\
         // returned path names where the parse failed (\"$\" for the whole body,\n\
         // \"$.field\" for a missing member), for the caller to build its own\n\
         // DecodeError; ok is false on any failure.\n\
         func {fn_name}(raw []byte) ({ty}, string, bool) {{\n\
         \tvar probe map[string]json.RawMessage\n\
         \tif err := json.Unmarshal(raw, &probe); err != nil {{\n\
         \t\treturn {ty}{{}}, \"$\", false\n\t}}\n\
         \tfor _, field := range {fields_var} {{\n\
         \t\tif rv, ok := probe[field]; !ok || string(rv) == \"null\" {{\n\
         \t\t\treturn {ty}{{}}, \"$.\" + field, false\n\t\t}}\n\t}}\n\
         \tvar out {ty}\n\
         \tif err := json.Unmarshal(raw, &out); err != nil {{\n\
         \t\treturn {ty}{{}}, \"$\", false\n\t}}\n\
         \treturn out, \"\", true\n\
         }}",
    );
    Some(Decl::raw_with(text, vec![import("json", "encoding/json")]))
}

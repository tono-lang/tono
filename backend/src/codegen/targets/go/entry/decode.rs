//! Decoding an operation's success payload into its declared output type.
//!
//! Shared by the two ways an operation gets a body: a protocol response through
//! the HTTP runtime, and a raw-form bespoke implementation returning an outcome.
//! Both carry the same thing (the JSON encoding of the declared output), so both
//! decode it the same way; only the Go expression naming the payload differs,
//! which is what [`Payload`] parameterizes.

use crate::codegen::conventions::wire_key;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::ir::{Module, Prim, ShapeKind, Tref};

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
/// or return the mapped decode failure. `fail` routes an error expression
/// through the bound `on_error` hook when the module has one; `refs` collects
/// the imports the emitted code needs.
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
            refs.push(import("json", "encoding/json"));
            let ty = go_type_label(t);
            let fail_decode = fail(format!(
                "&{decode}{{Path: \"$\", Expected: {ty:?}, Raw: {text}}}",
                decode = en.decode,
            ));
            let out_shape = match t {
                Tref::Ref { id, .. } => module.shapes.iter().find(|s| s.id == *id),
                _ => None,
            };
            let mut probe = String::new();
            if let Some(shape) = out_shape {
                if let ShapeKind::Structure { members, .. } = &shape.kind {
                    for m in members.iter().filter(|m| m.required) {
                        let name = wire_key(m);
                        // A missing required member points at that member (`$.tags`),
                        // not the whole body, so the caller sees which field the
                        // implementation omitted.
                        let fail_member = fail(format!(
                            "&{decode}{{Path: \"$.{name}\", Expected: {ty:?}, Raw: {text}}}",
                            decode = en.decode,
                        ));
                        probe.push_str(&format!(
                            "\tif rv, ok := probe[{name:?}]; !ok || string(rv) == \"null\" {{\n\t\treturn zero, {fail_member}\n\t}}\n",
                        ));
                    }
                }
            }
            if probe.is_empty() {
                format!(
                    "\tvar out {ty}\n\
                     \tif err := json.Unmarshal({bytes}, &out); err != nil {{\n\
                     \t\treturn zero, {fail_decode}\n\t}}\n\
                     \treturn out, nil",
                )
            } else {
                format!(
                    "\tvar probe map[string]json.RawMessage\n\
                     \tif err := json.Unmarshal({bytes}, &probe); err != nil {{\n\
                     \t\treturn zero, {fail_decode}\n\t}}\n\
                     {probe}\
                     \tvar out {ty}\n\
                     \tif err := json.Unmarshal({bytes}, &out); err != nil {{\n\
                     \t\treturn zero, {fail_decode}\n\t}}\n\
                     \treturn out, nil",
                )
            }
        }
        None => "\treturn nil".to_string(),
    }
}

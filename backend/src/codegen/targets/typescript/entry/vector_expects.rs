//! The expectation half of the generated Vitest tests: the outcome-pattern
//! and request-pattern assertions of `vector_tests`, split out so each file
//! stays within the repo size gate.

use crate::codegen::conventions::{rename_of, type_ident_from_id, wire_key};
use crate::codegen::declared_tests;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::codecs::encode_expr;
use crate::codegen::targets::typescript::types::LANG;
use crate::ir::{
    FieldPattern, Module, RequestPattern, ShapePattern, TaxonomyPattern, TestPattern, Tref,
};

use super::super::{field_camel_ren, literal, module_symbol};
use super::{json_text, ts_str, TestCtx};

/// Total wire equality: a structured output is compared in its wire spelling
/// (so `@rename` never leaks into the comparison), anything else as the
/// decoded value itself.
pub(super) fn eq_assert(
    t: &Tref,
    value: &serde_json::Value,
    refs: &mut Vec<Symbol>,
    module: &Module,
) -> String {
    match t {
        Tref::Ref { id, .. } => {
            let ty = type_ident_from_id(id);
            refs.push(module_symbol(&format!("encode{ty}"), module));
            format!("expect(encode{ty}(out)).toEqual({});\n", json_text(value))
        }
        _ => format!("expect(out).toEqual({});\n", literal(t, value)),
    }
}

/// Per-field checks over the output's wire form: equality, presence, absence;
/// a closed pattern also rejects unmentioned keys. A closed pattern of plain
/// `eq` fields pins the whole wire object, so it collapses into one total
/// comparison.
pub(super) fn struct_asserts(
    t: &Tref,
    pattern: &ShapePattern,
    refs: &mut Vec<Symbol>,
    module: &Module,
) -> String {
    if let Some(total) = declared_tests::closed_eq_object(pattern) {
        return eq_assert(t, &total, refs, module);
    }
    let encoded = match t {
        Tref::Ref { id, .. } => {
            let ty = type_ident_from_id(id);
            refs.push(module_symbol(&format!("encode{ty}"), module));
            format!("encode{ty}(out)")
        }
        // Validation restricts struct patterns to declared shapes; the wire
        // form of anything else is the value itself.
        _ => "out".to_string(),
    };
    let mut text = format!("const got = {encoded} as Record<string, unknown>;\n");
    text.push_str(&map_field_asserts("got", pattern));
    text
}

/// The per-field expects over an encoded wire map (keys in wire spelling).
pub(super) fn map_field_asserts(var: &str, pattern: &ShapePattern) -> String {
    let mut text = String::new();
    for (key, leaf) in &pattern.fields {
        let access = format!("{var}[{}]", ts_str(key));
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(value)) => {
                text.push_str(&format!(
                    "expect({access}).toEqual({});\n",
                    json_text(value)
                ));
            }
            FieldPattern::Present { .. } => {
                text.push_str(&format!("expect({access}).not.toBeUndefined();\n"));
            }
            FieldPattern::Absent { .. } => {
                text.push_str(&format!("expect({access}).toBeUndefined();\n"));
            }
            // Rejected by validation (nested structural patterns).
            FieldPattern::Pat(_) => {}
        }
    }
    if !pattern.open {
        let allowed: Vec<String> = pattern.fields.keys().map(|k| ts_str(k)).collect();
        text.push_str(&format!(
            "expect(\n  Object.entries({var})\n    .filter(([k, v]) => v !== undefined && ![{allowed}].includes(k))\n    .map(([k]) => k),\n).toEqual([]);\n",
            allowed = allowed.join(", "),
        ));
    }
    text
}

/// The assertions of a failure pattern over `caught`.
pub(super) fn failure_asserts(
    ctx: &TestCtx<'_>,
    pattern: &TestPattern,
    refs: &mut Vec<Symbol>,
) -> String {
    match pattern {
        TestPattern::Error(shape) => {
            let op = ctx.test.op.expect("an error pattern reads a call");
            let Some(err) = declared_tests::declared_error_by_shape(op, ctx.module, &shape.shape)
            else {
                // Unreachable when the tests passed validation.
                return format!(
                    "throw new Error({});\n",
                    ts_str(&format!(
                        "pattern names unknown error shape {}",
                        shape.shape
                    ))
                );
            };
            let ty = type_ident_from_id(&err.shape_id);
            let class = format!("{ty}Error");
            refs.push(module_symbol(&class, ctx.module));
            let mut text = format!(
                "expect(caught).toBeInstanceOf({class});\n\
                 const declared = caught as {class};\n"
            );
            text.push_str(&error_data_asserts(ctx, &err.shape_id, shape, refs));
            text
        }
        TestPattern::Taxonomy(tax) => taxonomy_asserts(ctx, tax, refs),
        // Success patterns never reach here.
        _ => String::new(),
    }
}

/// The per-field expects over a typed error's data: the pattern speaks wire
/// keys, the decoded object speaks the language spelling, so each named field
/// is re-encoded for the comparison. A closed pattern of plain `eq` fields
/// pins the whole wire object, so it collapses into one total comparison of
/// the re-encoded data.
pub(super) fn error_data_asserts(
    ctx: &TestCtx<'_>,
    shape_id: &str,
    pattern: &ShapePattern,
    refs: &mut Vec<Symbol>,
) -> String {
    if let Some(total) = declared_tests::closed_eq_object(pattern) {
        let ty = type_ident_from_id(shape_id);
        refs.push(module_symbol(&format!("encode{ty}"), ctx.module));
        return format!(
            "expect(encode{ty}(declared.data)).toEqual({});\n",
            json_text(&total)
        );
    }
    let members = ctx
        .module
        .shapes
        .iter()
        .find(|s| s.id == shape_id)
        .map(|s| match &s.kind {
            crate::ir::ShapeKind::Structure { members, .. } => members.clone(),
            _ => Vec::new(),
        })
        .unwrap_or_default();
    let mut text = String::new();
    let mut allowed: Vec<String> = Vec::new();
    for (key, leaf) in &pattern.fields {
        let Some(member) = members.iter().find(|m| wire_key(m) == *key) else {
            continue;
        };
        let ts_field = field_camel_ren(
            &member.name,
            rename_of(&member.traits, LANG).as_deref(),
            ctx.config,
        );
        allowed.push(ts_str(&ts_field));
        let access = format!("declared.data.{ts_field}");
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(value)) => {
                text.push_str(&format!(
                    "expect({encoded}).toEqual({want});\n",
                    encoded = encode_expr(&access, &member.target),
                    want = json_text(value),
                ));
            }
            FieldPattern::Present { .. } => {
                text.push_str(&format!("expect({access}).not.toBeUndefined();\n"));
            }
            FieldPattern::Absent { .. } => {
                text.push_str(&format!("expect({access}).toBeUndefined();\n"));
            }
            // Rejected by validation (nested structural patterns).
            FieldPattern::Pat(_) => {}
        }
    }
    if !pattern.open {
        text.push_str(&format!(
            "expect(\n  Object.entries(declared.data as unknown as Record<string, unknown>)\n    .filter(([k, v]) => v !== undefined && ![{allowed}].includes(k))\n    .map(([k]) => k),\n).toEqual([]);\n",
            allowed = allowed.join(", "),
        ));
    }
    text
}

pub(super) fn eq_str(leaf: &FieldPattern) -> Option<&str> {
    match leaf {
        FieldPattern::Pat(TestPattern::Eq(serde_json::Value::String(s))) => Some(s),
        _ => None,
    }
}

pub(super) fn eq_value(leaf: &FieldPattern) -> Option<&serde_json::Value> {
    match leaf {
        FieldPattern::Pat(TestPattern::Eq(value)) => Some(value),
        _ => None,
    }
}

/// A taxonomy pattern over `caught`: the failure must land in that category,
/// and the category's fields must match. `config` carries no structured
/// field, so its `field` equality checks the message names the member.
pub(super) fn taxonomy_asserts(
    ctx: &TestCtx<'_>,
    pattern: &TaxonomyPattern,
    refs: &mut Vec<Symbol>,
) -> String {
    let en = error_names();
    let bind = |ty: &str, refs: &mut Vec<Symbol>| {
        refs.push(module_symbol(ty, ctx.module));
        format!("expect(caught).toBeInstanceOf({ty});\n")
    };
    match pattern.category.as_str() {
        "api" => {
            let mut text = bind(&en.api, refs);
            text.push_str(&format!("const api = caught as {};\n", en.api));
            if let Some(status) = pattern.fields.get("status").and_then(eq_value) {
                text.push_str(&format!("expect(api.status).toBe({status});\n"));
            }
            if let Some(body) = pattern.fields.get("body").and_then(eq_str) {
                text.push_str(&format!("expect(api.body).toBe({});\n", ts_str(body)));
            }
            text
        }
        "validation" => {
            let mut text = bind(&en.validation, refs);
            if let Some(serde_json::Value::Array(fields)) =
                pattern.fields.get("fields").and_then(eq_value)
            {
                let want: Vec<String> = fields
                    .iter()
                    .filter_map(|f| f.as_str())
                    .map(ts_str)
                    .collect();
                text.push_str(&format!(
                    "expect((caught as {ty}).violations.map((x) => x.field)).toEqual([{want}]);\n",
                    ty = en.validation,
                    want = want.join(", "),
                ));
            }
            text
        }
        "decode" => {
            let mut text = bind(&en.decode, refs);
            if let Some(path) = pattern.fields.get("path").and_then(eq_str) {
                text.push_str(&format!(
                    "expect((caught as {ty}).path).toBe({path});\n",
                    ty = en.decode,
                    path = ts_str(path),
                ));
            }
            text
        }
        "contract" => {
            let mut text = bind(&en.contract, refs);
            if let Some(name) = pattern.fields.get("name").and_then(eq_str) {
                text.push_str(&format!(
                    "expect((caught as {ty}).contractName).toBe({name});\n",
                    ty = en.contract,
                    name = ts_str(name),
                ));
            }
            text
        }
        "config" => {
            let mut text = bind(&en.config, refs);
            if let Some(field) = pattern.fields.get("field").and_then(eq_str) {
                // The generated ConfigError carries a message naming the
                // member, not a structured field, so the check is containment.
                text.push_str(&format!(
                    "expect((caught as {ty}).message).toContain({field});\n",
                    ty = en.config,
                    field = ts_str(field),
                ));
            }
            text
        }
        "transport" => bind(&en.transport, refs),
        // Unreachable when the tests passed validation.
        other => format!(
            "throw new Error({});\n",
            ts_str(&format!("unknown error category {other}"))
        ),
    }
}

/// The `requests` expectations: the whole pattern list matches all recorded
/// requests, in order, with equal length (the length check is the retry count
/// assert), headers compared through one lowercased copy per request.
pub(super) fn request_asserts(patterns: &[RequestPattern]) -> String {
    let mut text = format!("expect(seen.length).toBe({});\n", patterns.len());
    for (i, pattern) in patterns.iter().enumerate() {
        let req = format!("req{i}");
        text.push_str(&format!("const {req} = seen[{i}];\n"));
        if let Some(m) = pattern.fields.get("method").and_then(eq_str) {
            text.push_str(&format!("expect({req}.method).toBe({});\n", ts_str(m)));
        }
        if let Some(p) = pattern.fields.get("path").and_then(eq_str) {
            text.push_str(&format!(
                "expect(new URL({req}.url).pathname).toBe({});\n",
                ts_str(p)
            ));
        }
        let headers: Vec<_> = pattern.headers.iter().flatten().collect();
        if !headers.is_empty() {
            text.push_str(&format!(
                "const lower{i} = Object.fromEntries(\n\
                 \x20 Object.entries({req}.headers).map(([k, v]) => [k.toLowerCase(), v] as const),\n\
                 );\n"
            ));
        }
        for (name, leaf) in headers {
            let key = ts_str(&name.to_ascii_lowercase());
            match leaf {
                FieldPattern::Present { .. } => {
                    text.push_str(&format!("expect(lower{i}[{key}]).not.toBeUndefined();\n"));
                }
                FieldPattern::Absent { .. } => {
                    text.push_str(&format!("expect(lower{i}[{key}]).toBeUndefined();\n"));
                }
                _ => {
                    if let Some(want) = eq_str(leaf) {
                        text.push_str(&format!(
                            "expect(lower{i}[{key}]).toBe({});\n",
                            ts_str(want)
                        ));
                    }
                }
            }
        }
    }
    text
}

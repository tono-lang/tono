//! The expectation half of the generated `cargo test` files: the
//! outcome-pattern and request-pattern assertions of `vector_tests`, split out
//! so each file stays within the repo size gate.

use crate::codegen::declared_tests;
use crate::codegen::ops::{error_names, error_type_name};
use crate::ir::{FieldPattern, RequestPattern, ShapePattern, TaxonomyPattern, TestPattern};

use super::{json_text, rust_string, TestCtx};

/// The assertions the outcome pattern dictates, over the bound `result`. No
/// pattern asserts bare success: the call (or construction) must at least not
/// fail.
pub(super) fn outcome_asserts(ctx: &TestCtx<'_>, has_output: bool) -> String {
    match ctx.test.outcome {
        None | Some(TestPattern::Ok(_)) => {
            "    result.map(|_| ()).expect(\"want ok\");\n".to_string()
        }
        Some(TestPattern::Eq(value)) => {
            if has_output {
                output_wire_eq_asserts(value)
            } else {
                // Unreachable when the tests passed validation.
                "    result.map(|_| ()).expect(\"want ok\");\n".to_string()
            }
        }
        Some(TestPattern::Struct(pattern)) => {
            // A closed pattern of plain `eq` fields pins the whole wire
            // object, so it collapses into one total comparison.
            if let Some(total) = declared_tests::closed_eq_object(pattern) {
                return output_wire_eq_asserts(&total);
            }
            let mut text = String::from(
                "    let out = result.expect(\"want ok\");\n\
                 \x20   let got = serde_json::to_value(&out).expect(\"encode output\");\n",
            );
            text.push_str(&map_field_asserts(pattern));
            text
        }
        Some(TestPattern::Error(pattern)) => error_asserts(ctx, pattern),
        Some(TestPattern::Taxonomy(pattern)) => taxonomy_asserts(pattern),
    }
}

/// One total comparison of the output's wire form against a pinned value.
fn output_wire_eq_asserts(value: &serde_json::Value) -> String {
    format!(
        "    let out = result.expect(\"want ok\");\n\
         \x20   let got = serde_json::to_value(&out).expect(\"encode output\");\n\
         \x20   let want: serde_json::Value =\n\
         \x20       serde_json::from_str({raw}).expect(\"decode expected output\");\n\
         \x20   assert_eq!(got, want, \"output mismatch\");\n",
        raw = rust_string(&json_text(value)),
    )
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

/// Per-field checks over the wire-form `got` value: equality in wire
/// spelling, presence, absence; a closed pattern also rejects unmentioned
/// keys.
pub(super) fn map_field_asserts(pattern: &ShapePattern) -> String {
    let mut text = String::new();
    for (key, leaf) in &pattern.fields {
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(value)) => {
                text.push_str(&format!(
                    "    let want: serde_json::Value =\n\
                     \x20       serde_json::from_str({raw}).expect(\"decode expected field\");\n\
                     \x20   assert_eq!(got.get({key:?}), Some(&want), \"field {key}\");\n",
                    raw = rust_string(&json_text(value)),
                ));
            }
            FieldPattern::Present { .. } => text.push_str(&format!(
                "    assert!(got.get({key:?}).is_some(), \"field {key} must be present\");\n"
            )),
            FieldPattern::Absent { .. } => text.push_str(&format!(
                "    assert!(got.get({key:?}).is_none(), \"field {key} must be absent\");\n"
            )),
            // Rejected by validation (nested structural patterns).
            FieldPattern::Pat(_) => {}
        }
    }
    if !pattern.open {
        let body = if pattern.fields.is_empty() {
            "            panic!(\"unexpected field {key}\");\n".to_string()
        } else {
            let arms: Vec<String> = pattern.fields.keys().map(|k| format!("{k:?}")).collect();
            format!(
                "            assert!(matches!(key.as_str(), {}), \"unexpected field {{key}}\");\n",
                arms.join(" | ")
            )
        };
        text.push_str(&format!(
            "    if let Some(object) = got.as_object() {{\n\
             \x20       for key in object.keys() {{\n\
             {body}\
             \x20       }}\n\
             \x20   }}\n"
        ));
    }
    text
}

/// A declared-error pattern: the failure must be that typed error, and its
/// wire form must satisfy the per-field checks.
pub(super) fn error_asserts(ctx: &TestCtx<'_>, pattern: &ShapePattern) -> String {
    let en = error_names();
    let op = ctx.test.op.expect("an error pattern reads a call");
    let Some(err) = declared_tests::declared_error_by_shape(op, ctx.module, &pattern.shape) else {
        // Unreachable when the tests passed validation.
        return format!(
            "    panic!(\"pattern names unknown error shape {}\");\n",
            pattern.shape
        );
    };
    let ty = error_type_name(&err);
    let shape = &pattern.shape;
    let reads_data = !pattern.fields.is_empty() || !pattern.open;
    let bind = if reads_data { "declared" } else { "_declared" };
    let mut text = format!(
        "    let failure = match result {{\n\
         \x20       Err({root}::Api(failure)) => failure,\n\
         \x20       Err(other) => panic!(\"want the declared error {shape}, got {{other:?}}\"),\n\
         \x20       Ok(_) => panic!(\"want the declared error {shape}, got ok\"),\n\
         \x20   }};\n\
         \x20   let {bind} = match failure {{\n\
         \x20       {failure}::{ty}(data) => data,\n\
         \x20       other => panic!(\"want the declared error {shape}, got {{other:?}}\"),\n\
         \x20   }};\n",
        root = en.root,
        failure = en.api_failure,
    );
    if let Some(total) = declared_tests::closed_eq_object(pattern) {
        // Closed with only `eq` fields: the whole wire object is pinned, so
        // the per-field checks collapse into one total comparison.
        text.push_str(&format!(
            "    let got = serde_json::to_value(&declared).expect(\"encode error data\");\n\
             \x20   let want: serde_json::Value =\n\
             \x20       serde_json::from_str({raw}).expect(\"decode expected error data\");\n\
             \x20   assert_eq!(got, want, \"error data mismatch\");\n",
            raw = rust_string(&json_text(&total)),
        ));
    } else if reads_data {
        text.push_str(
            "    let got = serde_json::to_value(&declared).expect(\"encode error data\");\n",
        );
        text.push_str(&map_field_asserts(pattern));
    }
    text
}

/// A taxonomy pattern: the failure must land in that category, and the
/// category's fields must match. `config` carries no structured field, so its
/// `field` equality checks the message names the member.
pub(super) fn taxonomy_asserts(pattern: &TaxonomyPattern) -> String {
    let en = error_names();
    match pattern.category.as_str() {
        "api" => {
            let mut text = format!(
                "    let api = match result {{\n\
                 \x20       Err({root}::Api({failure}::Undeclared(e))) => e,\n\
                 \x20       Err(other) => panic!(\"want the generic api error, got {{other:?}}\"),\n\
                 \x20       Ok(_) => panic!(\"want the generic api error, got ok\"),\n\
                 \x20   }};\n",
                root = en.root,
                failure = en.api_failure,
            );
            if let Some(status) = pattern.fields.get("status").and_then(eq_value) {
                text.push_str(&format!(
                    "    assert_eq!(api.status, {status}, \"api status\");\n"
                ));
            }
            if let Some(body) = pattern.fields.get("body").and_then(eq_str) {
                text.push_str(&format!(
                    "    assert_eq!(api.body, {body}, \"api body\");\n",
                    body = rust_string(body),
                ));
            }
            if pattern.fields.is_empty() {
                text.push_str("    let _ = api;\n");
            }
            text
        }
        "validation" => {
            let mut text = format!(
                "    let invalid = match result {{\n\
                 \x20       Err({root}::Validation(e)) => e,\n\
                 \x20       Err(other) => panic!(\"want a validation error, got {{other:?}}\"),\n\
                 \x20       Ok(_) => panic!(\"want a validation error, got ok\"),\n\
                 \x20   }};\n",
                root = en.root,
            );
            match pattern.fields.get("fields").and_then(eq_value) {
                Some(serde_json::Value::Array(fields)) => {
                    let want: Vec<String> = fields
                        .iter()
                        .filter_map(|f| f.as_str())
                        .map(|f| format!("{f:?}"))
                        .collect();
                    text.push_str(&format!(
                        "    let violated: Vec<&str> = invalid.violations.iter().map(|v| v.field.as_str()).collect();\n\
                         \x20   assert_eq!(violated, vec![{want}], \"violated fields\");\n",
                        want = want.join(", "),
                    ));
                }
                _ => text.push_str("    let _ = invalid;\n"),
            }
            text
        }
        "decode" => {
            let path_assert = match pattern.fields.get("path").and_then(eq_str) {
                Some(path) => format!("assert_eq!(e.path, {path:?}, \"decode path\")"),
                None => "drop(e)".to_string(),
            };
            format!(
                "    match result {{\n\
                 \x20       Err({root}::Decode(e)) => {path_assert},\n\
                 \x20       Err(other) => panic!(\"want a decode error, got {{other:?}}\"),\n\
                 \x20       Ok(_) => panic!(\"want a decode error, got ok\"),\n\
                 \x20   }}\n",
                root = en.root,
            )
        }
        "contract" => {
            let name_assert = match pattern.fields.get("name").and_then(eq_str) {
                Some(name) => format!("assert_eq!(e.contract_name, {name:?}, \"contract name\")"),
                None => "drop(e)".to_string(),
            };
            format!(
                "    match result {{\n\
                 \x20       Err({root}::Contract(e)) => {name_assert},\n\
                 \x20       Err(other) => panic!(\"want a contract error, got {{other:?}}\"),\n\
                 \x20       Ok(_) => panic!(\"want a contract error, got ok\"),\n\
                 \x20   }}\n",
                root = en.root,
            )
        }
        "config" => {
            // The generated ConfigError carries a message naming the member,
            // not a structured field, so the check is containment.
            let field_assert = match pattern.fields.get("field").and_then(eq_str) {
                Some(field) => format!(
                    "assert!(e.message.contains({field:?}), \"config error names {{}}\", e.message)"
                ),
                None => "drop(e)".to_string(),
            };
            format!(
                "    match result {{\n\
                 \x20       Err({root}::Config(e)) => {field_assert},\n\
                 \x20       Err(other) => panic!(\"want a config error, got {{other:?}}\"),\n\
                 \x20       Ok(_) => panic!(\"want a config error, got ok\"),\n\
                 \x20   }}\n",
                root = en.root,
            )
        }
        "transport" => format!(
            "    match result {{\n\
             \x20       Err({root}::Transport(_)) => {{}}\n\
             \x20       Err(other) => panic!(\"want a transport error, got {{other:?}}\"),\n\
             \x20       Ok(_) => panic!(\"want a transport error, got ok\"),\n\
             \x20   }}\n",
            root = en.root,
        ),
        // Unreachable when the tests passed validation.
        other => format!("    panic!(\"unknown error category {other}\");\n"),
    }
}

/// The `requests` expectations: the whole pattern list matches all recorded
/// requests, in order, with equal length (the length check is the retry count
/// assert), headers compared through one lowercased copy per request. The
/// path strips scheme/host/query inline: the generated crate carries no URL
/// parser dependency.
pub(super) fn request_asserts(patterns: &[RequestPattern]) -> String {
    let n = patterns.len();
    let mut text = format!(
        "    let seen = seen.lock().unwrap_or_else(|e| e.into_inner());\n\
         \x20   assert_eq!(seen.len(), {n}, \"recorded requests\");\n"
    );
    for (i, pattern) in patterns.iter().enumerate() {
        let req = format!("req{i}");
        text.push_str(&format!("    let {req} = &seen[{i}];\n"));
        if let Some(m) = pattern.fields.get("method").and_then(eq_str) {
            text.push_str(&format!(
                "    assert_eq!({req}.method, {m:?}, \"request {i} method\");\n"
            ));
        }
        if let Some(p) = pattern.fields.get("path").and_then(eq_str) {
            text.push_str(&format!(
                "    let rest{i} = {req}.url.split_once(\"://\").map_or({req}.url.as_str(), |(_, rest)| rest);\n\
                 \x20   let path{i} = rest{i}.find('/').map_or(\"/\", |at| &rest{i}[at..]);\n\
                 \x20   let path{i} = path{i}.find(['?', '#']).map_or(path{i}, |at| &path{i}[..at]);\n\
                 \x20   assert_eq!(path{i}, {p:?}, \"request {i} path\");\n"
            ));
        }
        let headers: Vec<_> = pattern.headers.iter().flatten().collect();
        if !headers.is_empty() {
            text.push_str(&format!(
                "    let lower{i}: std::collections::HashMap<String, String> = {req}\n\
                 \x20       .headers\n\
                 \x20       .iter()\n\
                 \x20       .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))\n\
                 \x20       .collect();\n"
            ));
        }
        for (name, leaf) in headers {
            let key = name.to_ascii_lowercase();
            match leaf {
                FieldPattern::Present { .. } => {
                    text.push_str(&format!(
                        "    assert!(lower{i}.contains_key({key:?}), \"request {i} header {name} must be present\");\n"
                    ));
                }
                FieldPattern::Absent { .. } => {
                    text.push_str(&format!(
                        "    assert!(!lower{i}.contains_key({key:?}), \"request {i} header {name} must be absent\");\n"
                    ));
                }
                _ => {
                    if let Some(want) = eq_str(leaf) {
                        text.push_str(&format!(
                            "    assert_eq!(lower{i}.get({key:?}).map(String::as_str).unwrap_or(\"\"), {want:?}, \"request {i} header {name}\");\n"
                        ));
                    }
                }
            }
        }
    }
    text
}

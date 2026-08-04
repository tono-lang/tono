//! The expectation half of the generated Go tests: the outcome-pattern and
//! request-pattern assertions of `vector_tests`, split out so each file stays
//! within the repo size gate. Every assertion is emitted as inline
//! straight-line code so the test files declare no helper functions.

use crate::codegen::declared_tests;
use crate::codegen::ops::{error_names, ErrorNames};
use crate::codegen::symbol::Symbol;
use crate::ir::{FieldPattern, RequestPattern, ShapePattern, TaxonomyPattern, TestPattern};

use super::super::import;
use super::{go_string, json_text, TestCtx};

pub(super) const OK_ASSERT: &str =
    "\tif err != nil {\n\t\tt.Fatalf(\"want ok, got error: %v\", err)\n\t}\n";

/// The assertions the outcome pattern dictates, over the bound `err` (and
/// `out` when the pattern reads the value). No pattern asserts bare success:
/// the call (or construction) must at least not fail.
pub(super) fn outcome_asserts(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    let en = error_names();
    match ctx.test.outcome {
        None | Some(TestPattern::Ok(_)) => OK_ASSERT.to_string(),
        Some(TestPattern::Eq(value)) => {
            format!(
                "{OK_ASSERT}{}",
                wire_eq_asserts("out", "output", value, refs)
            )
        }
        Some(TestPattern::Struct(pattern)) => {
            // A closed pattern of plain `eq` fields pins the whole wire
            // object, so it collapses into one total comparison.
            if let Some(total) = declared_tests::closed_eq_object(pattern) {
                return format!(
                    "{OK_ASSERT}{}",
                    wire_eq_asserts("out", "output", &total, refs)
                );
            }
            let mut text = format!("{OK_ASSERT}{}", wire_map_decode("out", "output", refs));
            text.push_str(&map_field_asserts(pattern, refs));
            text
        }
        Some(TestPattern::Error(pattern)) => error_asserts(ctx, pattern, refs),
        Some(TestPattern::Taxonomy(pattern)) => taxonomy_asserts(pattern, &en, refs),
    }
}

/// One total comparison in wire form: the subject is re-encoded and re-parsed
/// so the comparison sees data in its wire spelling, never language-side
/// identifiers.
fn wire_eq_asserts(
    subject: &str,
    what: &str,
    value: &serde_json::Value,
    refs: &mut Vec<Symbol>,
) -> String {
    refs.push(import("json", "encoding/json"));
    refs.push(import("reflect", "reflect"));
    format!(
        "\tblob, err := json.Marshal({subject})\n\
         \tif err != nil {{\n\t\tt.Fatalf(\"encode {what}: %v\", err)\n\t}}\n\
         \tvar got any\n\
         \tif err := json.Unmarshal(blob, &got); err != nil {{\n\
         \t\tt.Fatalf(\"decode encoded {what}: %v\", err)\n\t}}\n\
         \tvar want any\n\
         \tif err := json.Unmarshal([]byte({raw}), &want); err != nil {{\n\
         \t\tt.Fatalf(\"decode expected {what}: %v\", err)\n\t}}\n\
         \tif !reflect.DeepEqual(got, want) {{\n\
         \t\tt.Fatalf(\"{what} mismatch:\\n got %v\\nwant %v\", got, want)\n\t}}\n",
        raw = go_string(&json_text(value)),
    )
}

/// The subject's wire form bound as `got`, a `map[string]any` the per-field
/// checks read (a non-object subject fails the decode loudly).
fn wire_map_decode(subject: &str, what: &str, refs: &mut Vec<Symbol>) -> String {
    refs.push(import("json", "encoding/json"));
    format!(
        "\tblob, err := json.Marshal({subject})\n\
         \tif err != nil {{\n\t\tt.Fatalf(\"encode {what}: %v\", err)\n\t}}\n\
         \tvar got map[string]any\n\
         \tif err := json.Unmarshal(blob, &got); err != nil {{\n\
         \t\tt.Fatalf(\"decode encoded {what}: %v\", err)\n\t}}\n"
    )
}

/// Per-field checks over a `got` wire map: equality in wire spelling,
/// presence, absence; a closed pattern also rejects unmentioned keys.
pub(super) fn map_field_asserts(pattern: &ShapePattern, refs: &mut Vec<Symbol>) -> String {
    let mut text = String::new();
    for (key, leaf) in &pattern.fields {
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(value)) => {
                refs.push(import("json", "encoding/json"));
                refs.push(import("reflect", "reflect"));
                text.push_str(&format!(
                    "\t{{\n\
                     \t\tvar want any\n\
                     \t\tif err := json.Unmarshal([]byte({raw}), &want); err != nil {{\n\
                     \t\t\tt.Fatalf(\"decode expected field: %v\", err)\n\t\t}}\n\
                     \t\tif !reflect.DeepEqual(got[{key:?}], want) {{\n\
                     \t\t\tt.Errorf(\"field {key}: got %v, want %v\", got[{key:?}], want)\n\t\t}}\n\
                     \t}}\n",
                    raw = go_string(&json_text(value)),
                ));
            }
            FieldPattern::Present { .. } => text.push_str(&format!(
                "\tif _, ok := got[{key:?}]; !ok {{\n\t\tt.Errorf(\"field {key} must be present\")\n\t}}\n"
            )),
            FieldPattern::Absent { .. } => text.push_str(&format!(
                "\tif _, ok := got[{key:?}]; ok {{\n\t\tt.Errorf(\"field {key} must be absent\")\n\t}}\n"
            )),
            // Rejected by validation (nested structural patterns).
            FieldPattern::Pat(_) => {}
        }
    }
    if !pattern.open {
        let mut keys: Vec<String> = pattern.fields.keys().map(|k| format!("{k:?}")).collect();
        keys.sort();
        let arms = if keys.is_empty() {
            String::new()
        } else {
            format!("\t\tcase {}:\n", keys.join(", "))
        };
        text.push_str(&format!(
            "\tfor key := range got {{\n\
             \t\tswitch key {{\n\
             {arms}\
             \t\tdefault:\n\t\t\tt.Errorf(\"unexpected field %q\", key)\n\
             \t\t}}\n\
             \t}}\n"
        ));
    }
    text
}

/// A declared-error pattern: the failure must be that typed error, and its
/// wire form must satisfy the per-field checks (or, closed with only `eq`
/// fields, equal the whole pinned object).
pub(super) fn error_asserts(
    ctx: &TestCtx<'_>,
    pattern: &ShapePattern,
    refs: &mut Vec<Symbol>,
) -> String {
    refs.push(import("errors", "errors"));
    let op = ctx.test.op.expect("an error pattern reads a call");
    let Some(err) = declared_tests::declared_error_by_shape(op, ctx.module, &pattern.shape) else {
        // Unreachable when the tests passed validation.
        return format!(
            "\tt.Fatalf(\"pattern names unknown error shape {}\")\n",
            pattern.shape
        );
    };
    let ty = crate::codegen::conventions::type_ident_from_id(&err.shape_id);
    let mut text = format!(
        "\tvar declared *{ty}\n\
         \tif !errors.As(err, &declared) {{\n\t\tt.Fatalf(\"want the declared error {shape}, got %v\", err)\n\t}}\n",
        shape = pattern.shape,
    );
    if let Some(total) = declared_tests::closed_eq_object(pattern) {
        text.push_str(&wire_eq_asserts("declared", "error data", &total, refs));
    } else if !pattern.fields.is_empty() || !pattern.open {
        text.push_str(&wire_map_decode("declared", "error data", refs));
        text.push_str(&map_field_asserts(pattern, refs));
    }
    text
}

/// The single string equality a validated pattern leaf can carry.
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

/// A taxonomy pattern: the failure must land in that category, and the
/// category's fields must match. `config` carries no structured field, so its
/// `field` equality checks the message names the member.
pub(super) fn taxonomy_asserts(
    pattern: &TaxonomyPattern,
    en: &ErrorNames,
    refs: &mut Vec<Symbol>,
) -> String {
    refs.push(import("errors", "errors"));
    let bind = |ty: &str, what: &str, var: &str| {
        format!(
            "\tvar {var} *{ty}\n\
             \tif !errors.As(err, &{var}) {{\n\t\tt.Fatalf(\"want {what}, got %v\", err)\n\t}}\n"
        )
    };
    match pattern.category.as_str() {
        "api" => {
            let mut text = bind(&en.api, "the generic api error", "api");
            if let Some(status) = pattern.fields.get("status").and_then(eq_value) {
                text.push_str(&format!(
                    "\tif api.Status != {status} {{\n\t\tt.Errorf(\"api status: got %d, want {status}\", api.Status)\n\t}}\n"
                ));
            }
            if let Some(body) = pattern.fields.get("body").and_then(eq_str) {
                let lit = go_string(body);
                text.push_str(&format!(
                    "\tif api.Body != {lit} {{\n\t\tt.Errorf(\"api body: got %q, want %q\", api.Body, {lit})\n\t}}\n"
                ));
            }
            text
        }
        "validation" => {
            let mut text = bind(&en.validation, "a validation error", "invalid");
            if let Some(serde_json::Value::Array(fields)) =
                pattern.fields.get("fields").and_then(eq_value)
            {
                refs.push(import("reflect", "reflect"));
                let want: String = fields
                    .iter()
                    .filter_map(|f| f.as_str())
                    .map(|f| format!("{f:?}, "))
                    .collect();
                text.push_str(&format!(
                    "\tviolated := []string{{}}\n\
                     \tfor _, v := range invalid.Violations {{\n\t\tviolated = append(violated, v.Field)\n\t}}\n\
                     \tif want := []string{{{want}}}; !reflect.DeepEqual(violated, want) {{\n\
                     \t\tt.Errorf(\"violated fields: got %v, want %v\", violated, want)\n\t}}\n"
                ));
            }
            text
        }
        "decode" => {
            let mut text = bind(&en.decode, "a decode error", "bad");
            if let Some(path) = pattern.fields.get("path").and_then(eq_str) {
                text.push_str(&format!(
                    "\tif bad.Path != {path:?} {{\n\t\tt.Errorf(\"decode path: got %q, want {path}\", bad.Path)\n\t}}\n"
                ));
            }
            text
        }
        "contract" => {
            let mut text = bind(&en.contract, "a contract error", "broken");
            if let Some(name) = pattern.fields.get("name").and_then(eq_str) {
                text.push_str(&format!(
                    "\tif broken.ContractName != {name:?} {{\n\t\tt.Errorf(\"contract: got %q, want {name}\", broken.ContractName)\n\t}}\n"
                ));
            }
            text
        }
        "config" => {
            let mut text = bind(&en.config, "a config error", "cfg");
            if let Some(field) = pattern.fields.get("field").and_then(eq_str) {
                // The generated ConfigError carries a message naming the
                // member, not a structured field, so the check is containment.
                refs.push(import("strings", "strings"));
                text.push_str(&format!(
                    "\tif !strings.Contains(cfg.Message, {field:?}) {{\n\t\tt.Errorf(\"config error names %q, want {field}\", cfg.Message)\n\t}}\n"
                ));
            }
            text
        }
        "transport" => bind(&en.transport, "a transport error", "down"),
        // Unreachable when the tests passed validation.
        other => format!("\tt.Fatalf(\"unknown error category {other}\")\n"),
    }
}

/// The `requests` expectations: the whole pattern list matches all recorded
/// requests, in order, with equal length (the length check is the retry
/// count assert), headers compared through one lowercased copy per request.
pub(super) fn request_asserts(patterns: &[RequestPattern], refs: &mut Vec<Symbol>) -> String {
    let n = patterns.len();
    let mut text = format!(
        "\tif len(seen) != {n} {{\n\t\tt.Fatalf(\"recorded requests: got %d, want {n}\", len(seen))\n\t}}\n"
    );
    for (i, pattern) in patterns.iter().enumerate() {
        let req = format!("req{i}");
        text.push_str(&format!("\t{req} := seen[{i}]\n"));
        if let Some(m) = pattern.fields.get("method").and_then(eq_str) {
            text.push_str(&format!(
                "\tif {req}.Method != {m:?} {{\n\t\tt.Errorf(\"request {i} method: got %q, want {m}\", {req}.Method)\n\t}}\n"
            ));
        }
        if let Some(p) = pattern.fields.get("path").and_then(eq_str) {
            refs.push(import("url", "net/url"));
            text.push_str(&format!(
                "\tu{i}, err := url.Parse({req}.URL)\n\
                 \tif err != nil {{\n\t\tt.Fatalf(\"parse request url %q: %v\", {req}.URL, err)\n\t}}\n\
                 \tif u{i}.Path != {p:?} {{\n\t\tt.Errorf(\"request {i} path: got %q, want {p}\", u{i}.Path)\n\t}}\n"
            ));
        }
        let headers: Vec<_> = pattern.headers.iter().flatten().collect();
        if !headers.is_empty() {
            refs.push(import("strings", "strings"));
            text.push_str(&format!(
                "\tlower{i} := map[string]string{{}}\n\
                 \tfor k, v := range {req}.Headers {{\n\t\tlower{i}[strings.ToLower(k)] = v\n\t}}\n"
            ));
        }
        for (name, leaf) in headers {
            let key = name.to_ascii_lowercase();
            match leaf {
                FieldPattern::Present { .. } => text.push_str(&format!(
                    "\tif _, ok := lower{i}[{key:?}]; !ok {{\n\t\tt.Errorf(\"request {i} header {name} must be present\")\n\t}}\n"
                )),
                FieldPattern::Absent { .. } => text.push_str(&format!(
                    "\tif _, ok := lower{i}[{key:?}]; ok {{\n\t\tt.Errorf(\"request {i} header {name} must be absent\")\n\t}}\n"
                )),
                _ => {
                    if let Some(want) = eq_str(leaf) {
                        text.push_str(&format!(
                            "\tif lower{i}[{key:?}] != {want:?} {{\n\t\tt.Errorf(\"request {i} header {name}: got %q, want {want}\", lower{i}[{key:?}])\n\t}}\n"
                        ));
                    }
                }
            }
        }
    }
    text
}

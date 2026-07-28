//! Request assembly from a [`WireDescriptor`] and an operation's input
//! record. The descriptor is executed verbatim: every binding was resolved
//! once by the Protocol and frozen into the descriptor, so nothing here
//! re-derives a default (an unmarked member is already a body entry, a label
//! is already a label entry).

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::descriptor::{TemplatePart, ValueExpr, WireDescriptor};

/// Renders a decoded JSON value the way the wire expects it in a path, query,
/// or header position. An integral float must not print a trailing `.0`.
pub fn format_scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.to_string()
            } else if let Some(u) = n.as_u64() {
                u.to_string()
            } else {
                n.as_f64().map(|f| f.to_string()).unwrap_or_default()
            }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Substitutes each label binding into its `{name}` placeholder and each
/// `{.field}` placeholder with the resolved client value under that dotted
/// path in `values`. A path parameter must be present, so an absent or null
/// one substitutes empty rather than a literal `"null"`.
pub fn build_path(
    d: &WireDescriptor,
    record: &Map<String, Value>,
    values: &Map<String, Value>,
) -> String {
    let mut path = d.uri.clone();
    for b in &d.bindings {
        if b.part.kind != "label" {
            continue;
        }
        // A null value formats as the empty string (see format_scalar), the
        // same substitution an absent value gets, so there is no separate
        // presence check to spell here.
        let value = match record.get(&b.member) {
            Some(v) => percent_encode_path(&format_scalar(v)),
            None => String::new(),
        };
        path = path.replacen(&format!("{{{}}}", b.member), &value, 1);
    }
    substitute_field_placeholders(&path, values)
}

/// Replaces every `{.a.b}` run in a path template with the value under
/// `"a.b"` in the resolved client values, path-escaped. An absent or null
/// value substitutes empty, the same rule as an absent label. Consumes
/// `path` by shrinking slice, rather than an incrementing byte cursor, so a
/// multi-byte UTF-8 character never splits mid-codepoint.
fn substitute_field_placeholders(path: &str, values: &Map<String, Value>) -> String {
    let mut out = String::with_capacity(path.len());
    let mut rest = path;
    while !rest.is_empty() {
        if let Some(after_open) = rest.strip_prefix("{.") {
            if let Some(end) = after_open.find('}') {
                let key = &after_open[..end];
                if let Some(v) = values.get(key) {
                    out.push_str(&percent_encode_path(&format_scalar(v)));
                }
                rest = &after_open[end + 1..];
                continue;
            }
        }
        let ch = rest.chars().next().unwrap();
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Yields the operation's base URL from its endpoint field reference: the
/// string value under the dotted path in the resolved client values. An
/// absent declaration, absent value, or a non-string all yield `None` (the
/// caller falls back to `Options::base_url`).
pub fn resolve_endpoint(endpoint: &[String], values: &Map<String, Value>) -> Option<String> {
    if endpoint.is_empty() {
        return None;
    }
    let key = endpoint.join(".");
    match values.get(&key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

/// Renders template parts into one string: literal runs verbatim,
/// entry-field parts from the resolved client values, input parts from the
/// call's input record. `None` when any field/input part has no value (the
/// caller omits the position rather than emitting a half-rendered one).
pub fn resolve_template(
    parts: &[TemplatePart],
    values: &Map<String, Value>,
    record: &Map<String, Value>,
) -> Option<String> {
    let mut out = String::new();
    for p in parts {
        if let Some(lit) = &p.lit {
            out.push_str(lit);
            continue;
        }
        if let Some(field) = &p.field {
            let v = values.get(&field.join("."))?;
            if v.is_null() {
                return None;
            }
            out.push_str(&format_scalar(v));
            continue;
        }
        if let Some(input) = &p.input {
            let v = record.get(input)?;
            if v.is_null() {
                return None;
            }
            out.push_str(&format_scalar(v));
            continue;
        }
        return None;
    }
    Some(out)
}

/// Renders a descriptor value position: a literal verbatim, a field
/// reference from the resolved client values, or a template. `None` when the
/// referenced value is absent.
pub fn resolve_value_expr(
    e: &ValueExpr,
    values: &Map<String, Value>,
    record: &Map<String, Value>,
) -> Option<String> {
    if let Some(field) = &e.field {
        let v = values.get(&field.join("."))?;
        return if v.is_null() {
            None
        } else {
            Some(format_scalar(v))
        };
    }
    if let Some(template) = &e.template {
        return resolve_template(template, values, record);
    }
    if let Some(lit) = &e.lit {
        return Some(format_scalar(lit));
    }
    None
}

/// Resolves the operation's declared request headers. A header whose key or
/// value cannot resolve is omitted whole: half a header is worse than none,
/// and the declared chain's absence rules already ran in generated code.
pub fn declared_headers(
    d: &WireDescriptor,
    values: &Map<String, Value>,
    record: &Map<String, Value>,
) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for h in &d.request_headers {
        let Some(key) = resolve_template(&h.key, values, record) else {
            continue;
        };
        if key.is_empty() {
            continue;
        }
        let Some(value) = resolve_value_expr(&h.value, values, record) else {
            continue;
        };
        headers.insert(key, value);
    }
    headers
}

/// Serializes a query value as a repeated entry per element for a list, a
/// single entry otherwise; a null/absent value is omitted (the body's
/// nullable-omit rule, applied to the request line). Entries keep binding
/// order, matching the other runtimes.
pub fn build_query(d: &WireDescriptor, record: &Map<String, Value>) -> String {
    let mut entries = Vec::new();
    let mut add = |name: &str, value: &Value| {
        entries.push(format!(
            "{}={}",
            percent_encode_query(name),
            percent_encode_query(&format_scalar(value))
        ));
    };
    for b in &d.bindings {
        if b.part.kind != "query" {
            continue;
        }
        let Some(v) = record.get(&b.member) else {
            continue;
        };
        if v.is_null() {
            continue;
        }
        if let Some(list) = v.as_array() {
            for element in list {
                add(&b.part.name, element);
            }
        } else {
            add(&b.part.name, v);
        }
    }
    entries.join("&")
}

/// Layers the request headers: the operation's declared headers first, the
/// caller's base headers over them (a bespoke or caller-supplied header wins
/// over a declared one), and the input's header bindings last (the per-call
/// value is the most specific). A `before_request` hook sees the fully
/// layered result.
pub fn build_headers(
    d: &WireDescriptor,
    record: &Map<String, Value>,
    base: &HashMap<String, String>,
    values: &Map<String, Value>,
) -> HashMap<String, String> {
    let mut headers = declared_headers(d, values, record);
    for (k, v) in base {
        set_header(&mut headers, k, v.clone());
    }
    for b in &d.bindings {
        if b.part.kind != "header" {
            continue;
        }
        if let Some(v) = record.get(&b.member) {
            if !v.is_null() {
                set_header(&mut headers, &b.part.name, format_scalar(v));
            }
        }
    }
    headers
}

/// Overrides across casings: header names are case-insensitive, so a bespoke
/// `"authorization"` must replace a declared `"Authorization"` rather than
/// ride beside it.
pub fn set_header(headers: &mut HashMap<String, String>, name: &str, value: String) {
    headers.retain(|k, _| !k.eq_ignore_ascii_case(name));
    headers.insert(name.to_string(), value);
}

/// Whether a header is already set under any casing: a caller-supplied
/// `"Content-Type"` must suppress the default rather than sit beside a
/// second `"content-type"`.
pub fn has_header(headers: &HashMap<String, String>, name: &str) -> bool {
    headers.keys().any(|k| k.eq_ignore_ascii_case(name))
}

/// Produces the request body: a single payload member as the whole body,
/// otherwise the body members assembled into a JSON object in binding order
/// (or `None` when there are none). A member that is present but null still
/// lands in the object as null; only absence omits it.
pub fn build_body(d: &WireDescriptor, record: &Map<String, Value>) -> Option<Vec<u8>> {
    let mut fields = Map::new();
    for b in &d.bindings {
        if b.part.kind == "payload" {
            let v = record.get(&b.member)?;
            return Some(serde_json::to_vec(v).unwrap_or_default());
        }
        if b.part.kind != "body" {
            continue;
        }
        if let Some(v) = record.get(&b.member) {
            fields.insert(b.member.clone(), v.clone());
        }
    }
    if fields.is_empty() {
        return None;
    }
    Some(serde_json::to_vec(&Value::Object(fields)).unwrap_or_default())
}

/// Percent-encode one path segment's rendered value: reserved path delimiters
/// (`/`, `?`, `#`) and space are escaped, matching Go's `url.PathEscape`
/// closely enough for the wire contract (an already-safe character set is
/// left untouched).
fn percent_encode_path(s: &str) -> String {
    percent_encode(s, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
    })
}

fn percent_encode_query(s: &str) -> String {
    percent_encode(s, |b| {
        b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~')
    })
}

fn percent_encode(s: &str, is_safe: impl Fn(u8) -> bool) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        if is_safe(*b) {
            out.push(*b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn map(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn descriptor(json_text: &str) -> WireDescriptor {
        WireDescriptor::parse(json_text).unwrap()
    }

    #[test]
    fn format_scalar_prints_an_integral_float_with_no_trailing_zero() {
        assert_eq!(format_scalar(&json!(3.0)), "3");
        assert_eq!(format_scalar(&json!(3.5)), "3.5");
        assert_eq!(format_scalar(&json!("s")), "s");
        assert_eq!(format_scalar(&json!(true)), "true");
    }

    #[test]
    fn format_scalar_prints_a_huge_integral_float_in_full_no_saturation_no_exponent() {
        // Beyond i64::MAX, a naive `as i64` cast silently saturates; the wire
        // value must still print in full, matching Go's forced 'f' format.
        assert_eq!(format_scalar(&json!(1e20)), "100000000000000000000");
    }

    #[test]
    fn build_path_substitutes_a_label_and_escapes_it() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{id}","bindings":[["id",{"kind":"label"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let record = map(&[("id", json!("a b"))]);
        assert_eq!(build_path(&d, &record, &Map::new()), "/notes/a%20b");
    }

    #[test]
    fn build_path_substitutes_an_absent_label_as_empty() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{id}","bindings":[["id",{"kind":"label"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        assert_eq!(build_path(&d, &Map::new(), &Map::new()), "/notes/");
    }

    #[test]
    fn build_path_substitutes_an_explicit_null_label_as_empty() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{id}","bindings":[["id",{"kind":"label"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let record = map(&[("id", Value::Null)]);
        assert_eq!(build_path(&d, &record, &Map::new()), "/notes/");
    }

    #[test]
    fn build_path_substitutes_a_field_placeholder_from_values() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{.suffix}","bindings":[],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let values = map(&[("suffix", json!("s 1"))]);
        assert_eq!(build_path(&d, &Map::new(), &values), "/notes/s%201");
    }

    #[test]
    fn build_path_substitutes_an_explicit_null_field_placeholder_as_empty() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{.suffix}","bindings":[],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let values = map(&[("suffix", Value::Null)]);
        assert_eq!(build_path(&d, &Map::new(), &values), "/notes/");
    }

    #[test]
    fn field_placeholder_scan_resumes_correctly_after_the_match() {
        let values = map(&[("suffix", json!("x"))]);
        assert_eq!(
            substitute_field_placeholders("{.suffix}/extra", &values),
            "x/extra"
        );
    }

    #[test]
    fn field_placeholder_scan_resumes_correctly_after_a_multi_byte_character() {
        // A multi-byte trailing run after the substitution proves the scan
        // advances by whole characters, not raw bytes.
        let values = map(&[("suffix", json!("x"))]);
        assert_eq!(
            substitute_field_placeholders("{.suffix}\u{e9}!", &values),
            "x\u{e9}!"
        );
    }

    #[test]
    fn build_path_leaves_a_brace_run_with_no_leading_dot_verbatim() {
        // A `{name}` run that is not a bound label and does not open with `.`
        // is not a field placeholder; the substitution pass must not consume it.
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{unbound}","bindings":[],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        assert_eq!(build_path(&d, &Map::new(), &Map::new()), "/notes/{unbound}");
    }

    #[test]
    fn build_path_leaves_a_trailing_unterminated_brace_verbatim() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/notes/{","bindings":[],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        assert_eq!(build_path(&d, &Map::new(), &Map::new()), "/notes/{");
    }

    #[test]
    fn resolve_endpoint_falls_back_when_absent() {
        assert_eq!(resolve_endpoint(&[], &Map::new()), None);
        assert_eq!(resolve_endpoint(&["endpoint".into()], &Map::new()), None);
        let values = map(&[("endpoint", json!("https://acme.test"))]);
        assert_eq!(
            resolve_endpoint(&["endpoint".into()], &values),
            Some("https://acme.test".into())
        );
    }

    #[test]
    fn resolve_endpoint_falls_back_on_an_empty_string_value() {
        let values = map(&[("endpoint", json!(""))]);
        assert_eq!(resolve_endpoint(&["endpoint".into()], &values), None);
    }

    #[test]
    fn declared_headers_skips_a_header_with_an_unresolved_value() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[],"errors":[],"request_headers":[[[{"lit":"X-Client"}],{"field":["client_name"]}],[[{"lit":"X-Missing"}],{"field":["gone"]}]]}"#,
        );
        let values = map(&[("client_name", json!("demo"))]);
        let headers = declared_headers(&d, &values, &Map::new());
        assert_eq!(headers.get("X-Client"), Some(&"demo".to_string()));
        assert_eq!(headers.get("X-Missing"), None);
    }

    #[test]
    fn build_query_repeats_a_list_entry_per_element() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[["tags",{"kind":"query","name":"tag"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let record = map(&[("tags", json!(["a", "b"]))]);
        assert_eq!(build_query(&d, &record), "tag=a&tag=b");
    }

    #[test]
    fn build_headers_layers_declared_then_base_then_binding() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[["auth",{"kind":"header","name":"Authorization"}]],"response_bindings":[],"success":[],"errors":[],"request_headers":[[[{"lit":"Authorization"}],{"lit":"declared"}]]}"#,
        );
        let base: HashMap<String, String> =
            [("authorization".to_string(), "base".to_string())].into();
        let record = map(&[("auth", json!("per-call"))]);
        let headers = build_headers(&d, &record, &base, &Map::new());
        assert_eq!(headers.len(), 1);
        assert_eq!(
            headers.get("auth").or(headers.get("Authorization")),
            Some(&"per-call".to_string())
        );
    }

    #[test]
    fn build_body_marshals_body_members_in_declaration_order_preserving_null() {
        let d = descriptor(
            r#"{"http_method":"POST","uri":"/x","bindings":[["a",{"kind":"body","name":"a"}],["b",{"kind":"body","name":"b"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let record = map(&[("a", json!(1)), ("b", Value::Null)]);
        let body = build_body(&d, &record).unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!({"a": 1, "b": null}));
    }

    #[test]
    fn build_body_treats_a_payload_binding_as_the_whole_body() {
        let d = descriptor(
            r#"{"http_method":"POST","uri":"/x","bindings":[["note",{"kind":"payload"}]],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        let record = map(&[("note", json!({"text": "hi"}))]);
        let body = build_body(&d, &record).unwrap();
        let parsed: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed, json!({"text": "hi"}));
    }

    #[test]
    fn build_body_is_none_with_no_body_members() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[],"errors":[]}"#,
        );
        assert_eq!(build_body(&d, &Map::new()), None);
    }

    #[test]
    fn has_header_is_case_insensitive() {
        let headers: HashMap<String, String> =
            [("Content-Type".to_string(), "application/json".to_string())].into();
        assert!(has_header(&headers, "content-type"));
    }

    #[test]
    fn has_header_is_false_when_absent() {
        let headers: HashMap<String, String> =
            [("Accept".to_string(), "application/json".to_string())].into();
        assert!(!has_header(&headers, "content-type"));
    }
}

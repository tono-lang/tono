//! The inline HTTP transport: per-operation TypeScript built directly from a
//! `WireBinding`. The generated SDK carries its own transport and imports
//! nothing for it; the shared declaration text the emitted calls draw on
//! lives in [`super::transport_decls`].
//!
//! Poda by use happens at two granularities: the shared declarations are
//! pruned SDK-wide by the usual root-group mechanism (an SDK with no
//! `@retry` anywhere drops the backoff helpers entirely), while the retry
//! loop and the timeout wrapping are inlined directly into `op_call`'s own
//! text, gated on `wire.retry`/`wire.timeout`, so a single operation with
//! neither carries no trace of either in its own generated method.

use crate::codegen::entries::wire::{has_query, needs_record, success_test_expr};
use crate::codegen::symbol::Symbol;
use crate::ir::{TemplatePart, WireBinding, WireResponsePart, WireValue};

use super::support_symbol;

/// Indent every non-empty line of `text` by `by`, for text built at one
/// nesting depth that a caller embeds one level deeper (the retry loop wraps
/// the success block that a non-retrying operation leaves at the method's own
/// depth).
fn indent(text: &str, by: &str) -> String {
    text.lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{by}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A JS string literal, double-quoted (matches the target's own string
/// spelling elsewhere: `format!("{s:?}")` on a plain ASCII/UTF-8 string
/// produces the same escaping rules JSON and JS share).
fn js_str(s: &str) -> String {
    format!("{s:?}")
}

/// Render a parsed template (the `uri`, a `request_headers` key, or a
/// `WireValue::Template`) into a TypeScript expression: a single literal run
/// needs no template-literal wrapper, and a placeholder resolves either from
/// the resolved client settings (`Field`, via `field_expr`) or the call's own
/// record (`Input`).
fn template_expr(
    parts: &[TemplatePart],
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    if let [TemplatePart::Lit(s)] = parts {
        return js_str(s);
    }
    if parts.is_empty() {
        return js_str("");
    }
    let mut out = String::from("`");
    for part in parts {
        match part {
            TemplatePart::Lit(s) => {
                out.push_str(
                    &s.replace('\\', "\\\\")
                        .replace('`', "\\`")
                        .replace('$', "\\$"),
                );
            }
            TemplatePart::Field(path) => {
                out.push_str("${pathPart(");
                out.push_str(&field_expr(path));
                out.push_str(")}");
            }
            TemplatePart::Input(name) => {
                out.push_str("${pathPart(record[");
                out.push_str(&js_str(name));
                out.push_str("])}");
            }
            // [] is the whole typed parameter (read as `input_expr` directly,
            // no record needed); one segment is a member of the input
            // struct, exactly like the legacy `{name}` placeholder (an op
            // has exactly one parameter, so "member of the parameter" and
            // "member of the input" are the same decoded record). Deeper
            // paths are not reachable: the typechecker only resolves an
            // op-parameter reference one level deep.
            TemplatePart::Param(segs) => {
                out.push_str("${pathPart(");
                match segs.first() {
                    None => out.push_str(input_expr),
                    Some(name) => {
                        out.push_str("record[");
                        out.push_str(&js_str(name));
                        out.push(']');
                    }
                }
                out.push_str(")}");
            }
        }
    }
    out.push('`');
    out
}

/// A `WireValue` position (a `request_headers` value) rendered the same way a
/// template is, plus the two scalar forms.
fn wire_value_expr(
    v: &WireValue,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    match v {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => js_str(s),
            None => json.to_string(),
        },
        WireValue::Field(path) => format!("formatScalar({})", field_expr(path)),
        WireValue::Param(segs) => match segs.first() {
            None => format!("formatScalar({input_expr})"),
            Some(name) => format!("formatScalar(record[{}])", js_str(name)),
        },
        WireValue::Template(parts) => template_expr(parts, field_expr, input_expr),
        // Only ever emitted for @body; never a header/query/uri value.
        WireValue::Object(_) => {
            format!(
                "JSON.stringify({})",
                wire_value_native_expr(v, field_expr, input_expr)
            )
        }
    }
}

/// A `WireValue` rendered as a native TypeScript expression (not stringified):
/// the @body ctor mapper's field value, or nested inside one. Unlike
/// [`wire_value_expr`], this keeps the value's own shape so `JSON.stringify`
/// encodes it the same way it would encode the field directly.
fn wire_value_native_expr(
    v: &WireValue,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    match v {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => js_str(s),
            None => json.to_string(),
        },
        WireValue::Field(path) => field_expr(path),
        WireValue::Param(segs) => match segs.first() {
            None => input_expr.to_string(),
            Some(name) => format!("record[{}]", js_str(name)),
        },
        WireValue::Template(parts) => template_expr(parts, field_expr, input_expr),
        WireValue::Object(fields) => {
            let entries = fields
                .iter()
                .map(|(name, value)| {
                    format!(
                        "{}: {}",
                        js_str(name),
                        wire_value_native_expr(value, field_expr, input_expr)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {entries} }}")
        }
    }
}

/// The base-URL expression: the resolved endpoint value, concatenated with
/// the URI value by the caller. The frontend rejects an entry `@http` op
/// that does not name an endpoint, and `validate_entries` re-checks it at
/// generation time (IR can arrive from a file without ever passing the
/// frontend), so by emission time the binding always carries one and the
/// read needs no runtime guard.
fn endpoint_expr(
    wire: &WireBinding,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    let value = wire
        .endpoint
        .as_ref()
        .expect("validate_entries rejects an entry @http op with no endpoint");
    // The common case (a resolved entry-field endpoint) keeps its original
    // unwrapped spelling; the grammar's other forms (new with this task) go
    // through the general renderer.
    match value {
        WireValue::Field(path) => field_expr(path),
        other => wire_value_expr(other, field_expr, input_expr),
    }
}

fn uri_expr(
    wire: &WireBinding,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    match &wire.uri {
        WireValue::Template(parts) => template_expr(parts, field_expr, input_expr),
        other => wire_value_expr(other, field_expr, input_expr),
    }
}

/// One `setHeader(...)` call per declared `request_headers` entry.
fn declared_header_lines(
    wire: &WireBinding,
    indent_str: &str,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    wire.request_headers
        .iter()
        .map(|(key, value)| {
            format!(
                "{indent_str}setHeader(headers, {}, {});\n",
                template_expr(key, field_expr, input_expr),
                wire_value_expr(value, field_expr, input_expr)
            )
        })
        .collect()
}

/// One `appendQuery(...)` call per declared `@query` entry.
fn query_lines(
    wire: &WireBinding,
    indent_str: &str,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    wire.query
        .iter()
        .map(|(key, value)| {
            format!(
                "{indent_str}appendQuery(qs, {}, {});\n",
                template_expr(key, field_expr, input_expr),
                wire_value_native_expr(value, field_expr, input_expr)
            )
        })
        .collect()
}

/// The request body, or `None` when the operation sends no body: `wire.body`
/// says exactly what the body is, never inferred from what the input leaves
/// undeclared. The whole-parameter form stringifies the typed input directly
/// (correct even under `@wire` renames, and matching `needs_record`'s
/// decision that this case needs no `record` alias); every other form (one
/// member, an entry-field reference, a template, or the @body ctor mapper)
/// builds the native value first, so `JSON.stringify` encodes it the same
/// way it would encode the field directly.
fn body_expr(
    wire: &WireBinding,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> Option<String> {
    let body = wire.body.as_ref()?;
    if matches!(body, WireValue::Param(segs) if segs.is_empty()) {
        return Some(format!("JSON.stringify({input_expr})"));
    }
    Some(format!(
        "JSON.stringify({})",
        wire_value_native_expr(body, field_expr, input_expr)
    ))
}

/// The TypeScript spelling of the shared [`success_test_expr`] rule.
/// The `record` alias decision is shared too ([`needs_record`]): when every
/// bound member is a `Body` member, `body_expr`'s whole-body branch reads the
/// encoded input directly, so the alias (and its `as unknown as
/// Record<string, unknown>` cast) would be dead weight on the request.
fn success_expr(wire: &WireBinding) -> String {
    success_test_expr(wire, "response.status", "===")
}

/// The `outcome.body` expression: `response.body` verbatim when nothing folds
/// a response-bound member in (the common case), otherwise a ternary that
/// only folds on the success path (folding is a success-only concern, so an
/// error response passes its body through untouched either way).
fn outcome_body_expr(wire: &WireBinding) -> String {
    if wire.response_bindings.is_empty() {
        return "response.body".to_string();
    }
    format!(
        "({}) ? {} : response.body",
        success_expr(wire),
        response_fold_expr(wire)
    )
}

/// The success-path response body: a small IIFE that parses the body once,
/// sets each bound member, and re-serializes. Only called when
/// `response_bindings` is non-empty (see [`outcome_body_expr`]).
fn response_fold_expr(wire: &WireBinding) -> String {
    if wire.response_bindings.is_empty() {
        return "response.body".to_string();
    }
    let mut lines = String::new();
    for (name, part) in &wire.response_bindings {
        let value = match part {
            WireResponsePart::StatusCode => "response.status".to_string(),
            WireResponsePart::Header { name: header_name } => format!(
                "response.headers[{}] ?? null",
                js_str(&header_name.to_lowercase())
            ),
        };
        lines.push_str(&format!("      obj[{}] = {value};\n", js_str(name)));
    }
    format!(
        "(() => {{\n      const obj = parseJsonObject(response.body);\n{lines}      return JSON.stringify(obj);\n    }})()"
    )
}

/// The URL assembly lines: the query string builder only when a member is
/// query-bound, folded into `url` either way.
fn url_lines(
    wire: &WireBinding,
    indent_str: &str,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
) -> String {
    if !has_query(wire) {
        return format!(
            "{indent_str}const url = {} + {};\n",
            endpoint_expr(wire, field_expr, input_expr),
            uri_expr(wire, field_expr, input_expr)
        );
    }
    format!(
        "{indent_str}const qs = new URLSearchParams();\n\
         {q}\
         {indent_str}const url = {} + {}{tail};\n",
        endpoint_expr(wire, field_expr, input_expr),
        uri_expr(wire, field_expr, input_expr),
        q = query_lines(wire, indent_str, field_expr, input_expr),
        tail = " + (qs.toString() ? `?${qs.toString()}` : \"\")",
    )
}

/// The tail shared by the transport-failure catch and the declared-error
/// check: retry while attempts remain (and, for a declared error, while
/// `extra_cond` — its `retryable()` read — also holds), otherwise throw.
/// `has_retry: false` collapses to an unconditional throw (an error response
/// never retries when the operation declares no errors to classify it by, and
/// nothing retries when the operation declares no `@retry`).
fn retry_or_throw(
    indent_str: &str,
    has_retry: bool,
    extra_cond: Option<&str>,
    throw_expr: &str,
) -> String {
    if !has_retry {
        return format!("{indent_str}{throw_expr}\n");
    }
    let cond = match extra_cond {
        Some(c) => format!("attempt < maxRetries && {c}"),
        None => "attempt < maxRetries".to_string(),
    };
    format!(
        "{indent_str}if ({cond}) {{\n\
         {indent_str}  await retryDelay(attempt);\n\
         {indent_str}  continue;\n\
         {indent_str}}}\n\
         {indent_str}{throw_expr}\n"
    )
}

/// One lifecycle hook invocation, or nothing when the slot is unbound.
fn hook_line(indent_str: &str, bound: bool, slot: &str, var: &str) -> String {
    if !bound {
        return String::new();
    }
    format!("{indent_str}if (this.hooks.{slot}) {var} = await this.hooks.{slot}({var});\n")
}

/// One operation's transport call, replacing the descriptor-plus-`execute()`
/// call. Built once as a single "attempt" block, indented one level deeper
/// and wrapped in a retry loop when `wire.retry` is declared; a non-retrying
/// operation runs the identical text straight-line, so there is no separate
/// retrying/non-retrying code path to keep in sync.
#[allow(clippy::too_many_arguments)]
pub(super) fn op_call(
    wire: &WireBinding,
    method: &str,
    input_expr: &str,
    has_declared_errors: bool,
    discriminator: &str,
    error_line: &str,
    success_block: &str,
    transport_error: &str,
    throw: &dyn Fn(String) -> String,
    before_request_bound: bool,
    after_response_bound: bool,
    field_expr: &dyn Fn(&[String]) -> String,
    timeout_field_expr: &dyn Fn(&[String]) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    refs.push(support_symbol("HttpRequest"));
    refs.push(support_symbol("HttpResponse"));

    let has_retry = wire.retry.is_some();
    let has_timeout = wire.timeout.is_some();
    let body = body_expr(wire, field_expr, input_expr);
    let transport_throw = throw(format!("new {transport_error}(cause)"));

    let mut out = String::new();
    if needs_record(wire) {
        out.push_str(&format!(
            "    const record = {input_expr} as unknown as Record<string, unknown>;\n"
        ));
    }
    out.push_str(&url_lines(wire, "    ", field_expr, input_expr));
    out.push_str("    const headers: Record<string, string> = {};\n");
    out.push_str(&declared_header_lines(wire, "    ", field_expr, input_expr));
    out.push_str(
        "    for (const [k, v] of Object.entries(this.options.headers ?? {})) setHeader(headers, k, v);\n",
    );
    let body_field = match &body {
        Some(_) => "body".to_string(),
        None => "body: undefined".to_string(),
    };
    if let Some(b) = &body {
        out.push_str(&format!("    const body = {b};\n"));
        out.push_str(
            "    if (!hasHeader(headers, \"content-type\")) headers[\"content-type\"] = \"application/json\";\n",
        );
    }
    // A fresh `headers` copy per request literal: a `before_request` hook may
    // mutate the object it receives in place rather than returning a new one,
    // and a retried attempt must not see a prior attempt's mutation (the
    // runtime this replaces rebuilds headers fresh on every attempt).
    let request_literal = format!(
        "{{ method: {}, url, headers: {{ ...headers }}, {body_field} }}",
        js_str(method)
    );
    let send_call = if has_timeout {
        let path = wire.timeout.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "    const timeoutMs = {};\n",
            timeout_field_expr(path)
        ));
        "httpSendWithTimeout(this.options, request, timeoutMs)".to_string()
    } else {
        "httpSend(this.options, request, undefined)".to_string()
    };
    if has_retry {
        let path = wire.retry.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "    const maxRetries = resolveMaxRetries({});\n",
            field_expr(path)
        ));
    }

    // The per-attempt body: `d` is its own statement depth, one level deeper
    // than the method (`"    "`) when a retry loop wraps it.
    let d = if has_retry { "      " } else { "    " };
    let request_kw = if before_request_bound { "let" } else { "const" };
    let mut attempt = String::new();
    attempt.push_str(&format!(
        "{d}{request_kw} request: HttpRequest = {request_literal};\n"
    ));
    attempt.push_str(&hook_line(
        d,
        before_request_bound,
        "before_request",
        "request",
    ));
    attempt.push_str(&format!("{d}let response: HttpResponse;\n"));
    attempt.push_str(&format!("{d}try {{\n"));
    attempt.push_str(&format!("{d}  response = await {send_call};\n"));
    attempt.push_str(&format!("{d}}} catch (cause) {{\n"));
    attempt.push_str(&retry_or_throw(
        &format!("{d}  "),
        has_retry,
        None,
        &transport_throw,
    ));
    attempt.push_str(&format!("{d}}}\n"));
    attempt.push_str(&hook_line(
        d,
        after_response_bound,
        "after_response",
        "response",
    ));
    // `outcome` is the name `success_block`/`error_line` (built in decode.rs)
    // reads `.status`/`.body` off — a name shared with the raw-bespoke impl
    // path, whose own `outcome` is a genuinely different shape (`{success,
    // status, code, body}`) that decode.rs's generic text has to fit both
    // without knowing which one it is. With nothing to fold in, `response`
    // already has the `.status`/`.body` shape that name needs, so it stands
    // in directly (not a leftover alias) rather than being reconstructed
    // field by field.
    if wire.response_bindings.is_empty() {
        attempt.push_str(&format!("{d}const outcome = response;\n"));
    } else {
        attempt.push_str(&format!(
            "{d}const outcome = {{ status: response.status, body: {} }};\n",
            outcome_body_expr(wire),
        ));
    }
    // The success path is control-flow-terminal (`success_block` always
    // returns or throws), so the error path below it needs no `else`: it is
    // only ever reached once the response missed.
    attempt.push_str(&format!("{d}if ({}) {{\n", success_expr(wire)));
    attempt.push_str(&indent(
        success_block,
        &" ".repeat(d.len().saturating_sub(2)),
    ));
    attempt.push('\n');
    attempt.push_str(&format!("{d}}}\n"));
    if has_declared_errors {
        attempt.push_str(&format!(
            "{d}const err = {discriminator}(outcome.status, outcome.body);\n"
        ));
        attempt.push_str(&retry_or_throw(
            d,
            has_retry,
            Some("err.retryable()"),
            &throw("err".to_string()),
        ));
    } else {
        attempt.push_str(&retry_or_throw(d, false, None, error_line));
    }

    if has_retry {
        out.push_str("    for (let attempt = 0; ; attempt++) {\n");
        out.push_str(&attempt);
        out.push_str("    }\n");
    } else {
        out.push_str(&attempt);
    }
    out
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

//! The inline HTTP transport: per-operation TypeScript built directly from a
//! `WireBinding` (IR v8), plus the small set of shared declarations every
//! operation's generated call draws on. Replaces the opaque `wire_descriptor`
//! blob and the `execute()` call into `@tono/http-runtime-ts`: the generated
//! SDK now carries its own transport and imports nothing for it.
//!
//! Poda by use happens at two granularities: `internal_helpers()`'s
//! declarations are pruned SDK-wide by the usual root-group mechanism (an SDK
//! with no `@retry` anywhere drops the backoff helpers entirely), while the
//! retry loop and the timeout wrapping are inlined directly into `op_call`'s
//! own text, gated on `wire.retry`/`wire.timeout`, so a single operation with
//! neither carries no trace of either in its own generated method.

use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{TemplatePart, WireBinding, WirePart, WireResponsePart, WireValue};

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

fn values_lookup(path: &[String]) -> String {
    format!("this.options.values?.[{}]", js_str(&path.join(".")))
}

/// Render a parsed template (the `uri`, a `request_headers` key, or a
/// `WireValue::Template`) into a TypeScript expression: a single literal run
/// needs no template-literal wrapper, and a placeholder resolves either from
/// the resolved client values (`Field`) or the call's own record (`Input`).
fn template_expr(parts: &[TemplatePart]) -> String {
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
                out.push_str(&values_lookup(path));
                out.push_str(")}");
            }
            TemplatePart::Input(name) => {
                out.push_str("${pathPart(record[");
                out.push_str(&js_str(name));
                out.push_str("])}");
            }
        }
    }
    out.push('`');
    out
}

/// A `WireValue` position (a `request_headers` value) rendered the same way a
/// template is, plus the two scalar forms.
fn wire_value_expr(v: &WireValue) -> String {
    match v {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => js_str(s),
            None => json.to_string(),
        },
        WireValue::Field(path) => format!("formatScalar({})", values_lookup(path)),
        WireValue::Template(parts) => template_expr(parts),
    }
}

/// The path expression: `this.options.baseUrl` (or the resolved endpoint
/// field) concatenated with the URI template.
fn endpoint_expr(wire: &WireBinding) -> String {
    match &wire.endpoint {
        None => "this.options.baseUrl".to_string(),
        Some(path) => {
            let v = values_lookup(path);
            format!(
                "(typeof {v} === \"string\" && {v} !== \"\" ? ({v} as string) : this.options.baseUrl)"
            )
        }
    }
}

fn uri_expr(wire: &WireBinding) -> String {
    template_expr(&wire.uri)
}

/// One `setHeader(...)` call per declared `request_headers` entry.
fn declared_header_lines(wire: &WireBinding, indent_str: &str) -> String {
    wire.request_headers
        .iter()
        .map(|(key, value)| {
            format!(
                "{indent_str}setHeader(headers, {}, {});\n",
                template_expr(key),
                wire_value_expr(value)
            )
        })
        .collect()
}

/// One `setHeader(...)` call per input member bound to a header position.
fn per_call_header_lines(wire: &WireBinding, indent_str: &str) -> String {
    wire.bindings
        .iter()
        .filter_map(|(name, part)| match part {
            WirePart::Header { name: header_name } => Some(format!(
                "{indent_str}setHeader(headers, {}, formatScalar(record[{}]));\n",
                js_str(header_name),
                js_str(name)
            )),
            _ => None,
        })
        .collect()
}

/// Whether any input member is bound to the query string.
fn has_query(wire: &WireBinding) -> bool {
    wire.bindings
        .values()
        .any(|p| matches!(p, WirePart::Query { .. }))
}

/// One `appendQuery(...)` call per input member bound to the query string.
fn query_lines(wire: &WireBinding, indent_str: &str) -> String {
    wire.bindings
        .iter()
        .filter_map(|(name, part)| match part {
            WirePart::Query { name: query_name } => Some(format!(
                "{indent_str}appendQuery(qs, {}, record[{}]);\n",
                js_str(query_name),
                js_str(name)
            )),
            _ => None,
        })
        .collect()
}

/// The request body: a `Payload`-kind member wins outright (mirrors the
/// runtime's current early-return semantics), otherwise a compile-time-known
/// object literal of the `Body`-kind members. `None` when the operation sends
/// no body at all.
fn body_expr(wire: &WireBinding) -> Option<String> {
    if let Some((name, _)) = wire
        .bindings
        .iter()
        .find(|(_, p)| matches!(p, WirePart::Payload))
    {
        return Some(format!("JSON.stringify(record[{}])", js_str(name)));
    }
    let fields: Vec<&String> = wire
        .bindings
        .iter()
        .filter(|(_, p)| matches!(p, WirePart::Body))
        .map(|(name, _)| name)
        .collect();
    if fields.is_empty() {
        return None;
    }
    let object = fields
        .iter()
        .map(|name| format!("{}: record[{}]", js_str(name), js_str(name)))
        .collect::<Vec<_>>()
        .join(", ");
    Some(format!("JSON.stringify({{ {object} }})"))
}

/// Whether the operation's input carries any member a request position reads
/// (so a `record` alias of the encoded input is needed at all).
fn needs_record(wire: &WireBinding) -> bool {
    !wire.bindings.is_empty()
}

/// `response.status >= 200 && response.status < 300`, plus one `||` arm per
/// declared success status outside the 2xx range: any 2xx succeeds even when
/// not literally declared, matching the runtime's current `isSuccessStatus`.
fn success_expr(wire: &WireBinding) -> String {
    let mut out = String::from("response.status >= 200 && response.status < 300");
    for code in &wire.success {
        if !(200..300).contains(code) {
            out.push_str(&format!(" || response.status === {code}"));
        }
    }
    out
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
fn url_lines(wire: &WireBinding, indent_str: &str) -> String {
    if !has_query(wire) {
        return format!(
            "{indent_str}const url = {} + {};\n",
            endpoint_expr(wire),
            uri_expr(wire)
        );
    }
    format!(
        "{indent_str}const qs = new URLSearchParams();\n\
         {q}\
         {indent_str}const url = {} + {}{tail};\n",
        endpoint_expr(wire),
        uri_expr(wire),
        q = query_lines(wire, indent_str),
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
    refs: &mut Vec<Symbol>,
) -> String {
    refs.push(support_symbol("HttpRequest"));
    refs.push(support_symbol("HttpResponse"));

    let has_retry = wire.retry.is_some();
    let has_timeout = wire.timeout.is_some();
    let body = body_expr(wire);
    let transport_throw = throw(format!("new {transport_error}(cause)"));

    let mut out = String::new();
    if needs_record(wire) {
        out.push_str(&format!(
            "    const record = {input_expr} as unknown as Record<string, unknown>;\n"
        ));
    }
    out.push_str(&url_lines(wire, "    "));
    out.push_str("    const headers: Record<string, string> = {};\n");
    out.push_str(&declared_header_lines(wire, "    "));
    out.push_str(
        "    for (const [k, v] of Object.entries(this.options.headers ?? {})) setHeader(headers, k, v);\n",
    );
    out.push_str(&per_call_header_lines(wire, "    "));
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
            "    const timeoutMs = resolveTimeoutMs({});\n",
            values_lookup(path)
        ));
        "httpSendWithTimeout(this.options, request, timeoutMs)".to_string()
    } else {
        "httpSend(this.options, request, undefined)".to_string()
    };
    if has_retry {
        let path = wire.retry.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "    const maxRetries = resolveMaxRetries({});\n",
            values_lookup(path)
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
    attempt.push_str(&format!(
        "{d}const outcome = {{ status: response.status, body: {} }};\n",
        outcome_body_expr(wire),
    ));
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

/// The public, bespoke-facing transport types (`Group::root_support()`): the
/// request/response shapes a bound `before_request`/`after_response` hook
/// type-checks against, plus the construction-time `ClientOptions`/`Hooks`.
pub(crate) fn http_support_decls() -> Vec<Decl> {
    vec![
        Decl::raw_providing(
            "HttpResponse",
            "// HttpResponse is the response the runtime reads before classifying it.\n\
             // An after_response hook may return a mutated copy.\n\
             export interface HttpResponse {\n  status: number;\n  headers: Record<string, string>;\n  body: string;\n}",
            Vec::new(),
        ),
        Decl::raw_providing(
            "HttpRequest",
            "// HttpRequest is the request the runtime builds before sending it. A\n\
             // before_request hook receives this and may return a mutated copy (set an\n\
             // auth header, sign the body).\n\
             export interface HttpRequest {\n  method: string;\n  url: string;\n  headers: Record<string, string>;\n  body: string | undefined;\n}",
            Vec::new(),
        ),
        Decl::raw_providing(
            "HttpTransport",
            "// HttpTransport adapts any HTTP stack by mapping HttpRequest/HttpResponse,\n\
             // without emulating fetch. One call is one attempt: the generated client\n\
             // owns retry, so a transport with internal retries does not combine with\n\
             // it.\n\
             export type HttpTransport = (req: HttpRequest, signal?: AbortSignal) => Promise<HttpResponse>;",
            vec![support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
        Decl::raw_providing(
            "ClientOptions",
            "// ClientOptions is what the caller supplies once, shared across every\n\
             // operation. Exactly one transport slot may be set: fetch (native) or\n\
             // transport (canonical); setting both is a construction error. No slot\n\
             // ships its own auth; a bespoke hook sets an auth header through headers.\n\
             export interface ClientOptions {\n  readonly baseUrl: string;\n  readonly fetch?: typeof fetch;\n  readonly transport?: HttpTransport;\n  readonly headers?: Readonly<Record<string, string>>;\n  readonly values?: Readonly<Record<string, unknown>>;\n}",
            vec![support_symbol("HttpTransport")],
        ),
        Decl::raw_providing(
            "Hooks",
            "// Hooks are the lifecycle slots the generated client invokes around the\n\
             // transport. A hook that throws propagates raw; the generated wrapper is\n\
             // what turns it into a ContractError.\n\
             export interface Hooks {\n  readonly before_request?: (req: HttpRequest) => HttpRequest | Promise<HttpRequest>;\n  readonly after_response?: (res: HttpResponse) => HttpResponse | Promise<HttpResponse>;\n}",
            vec![support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
    ]
}

/// The internal transport helpers (`Group::root("http")`), pruned SDK-wide by
/// usage exactly like the `duration`/`casing` groups: an SDK with no
/// `@retry` anywhere never carries the backoff helpers.
pub(crate) fn internal_helpers() -> Vec<Decl> {
    vec![
        Decl::raw_providing(
            "formatScalar",
            "// formatScalar renders a value the way the wire expects it in a path,\n\
             // query, or header position: strings verbatim, everything else as JSON.\n\
             export function formatScalar(value: unknown): string {\n\
             \x20 if (typeof value === \"string\") return value;\n\
             \x20 try {\n\
             \x20   return JSON.stringify(value) ?? \"\";\n\
             \x20 } catch {\n\
             \x20   return \"\";\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "pathPart",
            "// pathPart renders a path segment: an absent value substitutes empty\n\
             // rather than the literal \"undefined\"/\"null\".\n\
             export function pathPart(value: unknown): string {\n\
             \x20 return value === undefined || value === null ? \"\" : encodeURIComponent(formatScalar(value));\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "setHeader",
            "// setHeader overrides across casings: header names are case-insensitive,\n\
             // so a bespoke \"authorization\" replaces a declared \"Authorization\" rather\n\
             // than riding beside it.\n\
             export function setHeader(headers: Record<string, string>, name: string, value: string): void {\n\
             \x20 const lower = name.toLowerCase();\n\
             \x20 for (const key of Object.keys(headers)) {\n\
             \x20   if (key.toLowerCase() === lower) delete headers[key];\n\
             \x20 }\n\
             \x20 headers[name] = value;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "hasHeader",
            "export function hasHeader(headers: Record<string, string>, name: string): boolean {\n\
             \x20 const lower = name.toLowerCase();\n\
             \x20 return Object.keys(headers).some((key) => key.toLowerCase() === lower);\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "headerRecord",
            "// headerRecord collects a Headers object into a plain record so a hook can\n\
             // read and rewrite it without a live Headers instance.\n\
             export function headerRecord(headers: Headers): Record<string, string> {\n\
             \x20 const record: Record<string, string> = {};\n\
             \x20 headers.forEach((value, key) => {\n\
             \x20   record[key] = value;\n\
             \x20 });\n\
             \x20 return record;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "appendQuery",
            "// appendQuery serializes as a repeated entry per element for a list, a\n\
             // single entry otherwise; a null/absent value is omitted.\n\
             export function appendQuery(qs: URLSearchParams, name: string, value: unknown): void {\n\
             \x20 if (value === undefined || value === null) return;\n\
             \x20 if (Array.isArray(value)) {\n\
             \x20   for (const element of value) qs.append(name, String(element));\n\
             \x20 } else {\n\
             \x20   qs.append(name, String(value));\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "parseJsonObject",
            "// parseJsonObject parses a response body for response-bound member\n\
             // folding; a non-object or unparsable body leaves the bound fields to\n\
             // stand on their own.\n\
             export function parseJsonObject(body: string): Record<string, unknown> {\n\
             \x20 if (body === \"\") return {};\n\
             \x20 try {\n\
             \x20   const parsed: unknown = JSON.parse(body);\n\
             \x20   return parsed !== null && typeof parsed === \"object\" ? (parsed as Record<string, unknown>) : {};\n\
             \x20 } catch {\n\
             \x20   return {};\n\
             \x20 }\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "assertExclusiveTransport",
            "// assertExclusiveTransport rejects setting both transport slots: the\n\
             // caller must pick the native slot (fetch) or the canonical one\n\
             // (transport), not both.\n\
             export function assertExclusiveTransport(options: ClientOptions): void {\n\
             \x20 if (options.fetch && options.transport) {\n\
             \x20   throw new Error(\n\
             \x20     \"ClientOptions.fetch and ClientOptions.transport are mutually exclusive: set the native slot or the canonical slot, not both\",\n\
             \x20   );\n\
             \x20 }\n\
             }",
            vec![support_symbol("ClientOptions")],
        ),
        Decl::raw_providing(
            "httpSend",
            "// httpSend performs one attempt: the canonical transport when set,\n\
             // otherwise fetch.\n\
             export async function httpSend(\n\
             \x20 options: ClientOptions,\n\
             \x20 request: HttpRequest,\n\
             \x20 signal: AbortSignal | undefined,\n\
             ): Promise<HttpResponse> {\n\
             \x20 if (options.transport) return options.transport(request, signal);\n\
             \x20 const transport = options.fetch ?? fetch;\n\
             \x20 const response = await transport(request.url, {\n\
             \x20   method: request.method,\n\
             \x20   headers: request.headers,\n\
             \x20   body: request.body,\n\
             \x20   signal,\n\
             \x20 });\n\
             \x20 const text = await response.text();\n\
             \x20 return { status: response.status, headers: headerRecord(response.headers), body: text };\n\
             }",
            vec![support_symbol("ClientOptions"), support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
        Decl::raw_providing(
            "httpSendWithTimeout",
            "// httpSendWithTimeout bounds one attempt: the timeout aborts the signal\n\
             // (so a cooperating transport cancels its work) and rejects the attempt\n\
             // regardless, so a transport that ignores the signal still times out.\n\
             export async function httpSendWithTimeout(\n\
             \x20 options: ClientOptions,\n\
             \x20 request: HttpRequest,\n\
             \x20 timeoutMs: number,\n\
             ): Promise<HttpResponse> {\n\
             \x20 if (timeoutMs <= 0) return httpSend(options, request, undefined);\n\
             \x20 const controller = new AbortController();\n\
             \x20 let timer: ReturnType<typeof setTimeout> | undefined;\n\
             \x20 const expiry = new Promise<never>((_, reject) => {\n\
             \x20   timer = setTimeout(() => {\n\
             \x20     const cause = new Error(`attempt timed out after ${timeoutMs}ms`);\n\
             \x20     controller.abort(cause);\n\
             \x20     reject(cause);\n\
             \x20   }, timeoutMs);\n\
             \x20 });\n\
             \x20 const call = httpSend(options, request, controller.signal);\n\
             \x20 try {\n\
             \x20   const raced = Promise.race([call, expiry]);\n\
             \x20   call.catch(() => {});\n\
             \x20   return await raced;\n\
             \x20 } finally {\n\
             \x20   clearTimeout(timer);\n\
             \x20 }\n\
             }",
            vec![support_symbol("ClientOptions"), support_symbol("HttpRequest"), support_symbol("HttpResponse")],
        ),
        Decl::raw_providing(
            "backoffDelayMs",
            "// backoffDelayMs is exponential with full jitter: the constants are part\n\
             // of the cross-runtime parity contract and must match every other target.\n\
             export function backoffDelayMs(attempt: number, random: number): number {\n\
             \x20 return random * Math.min(2000, 100 * 2 ** attempt);\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "resolveMaxRetries",
            "// resolveMaxRetries reads the resolved client value the descriptor's\n\
             // retry field names: a non-numeric value or one below one both mean zero\n\
             // retries; a fractional value floors.\n\
             export function resolveMaxRetries(value: unknown): number {\n\
             \x20 return typeof value === \"number\" && Number.isFinite(value) && value >= 1 ? Math.floor(value) : 0;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "resolveTimeoutMs",
            "// resolveTimeoutMs reads the resolved client value the descriptor's\n\
             // timeout field names: a non-numeric value means no deadline.\n\
             export function resolveTimeoutMs(value: unknown): number {\n\
             \x20 return typeof value === \"number\" && Number.isFinite(value) ? value : 0;\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "defaultSleep",
            "// defaultSleep and defaultRandom are the timing seam: internal to the\n\
             // package (this group is excluded from package.json's exports map), so\n\
             // nothing outside the generated SDK can reach or override them.\n\
             export function defaultSleep(ms: number): Promise<void> {\n\
             \x20 return new Promise((resolve) => setTimeout(resolve, ms));\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "defaultRandom",
            "// Math.random seeds only the retry backoff's jitter, never anything\n\
             // security-sensitive (no token, no session id, no cryptographic use),\n\
             // so a predictable PRNG is fine here.\n\
             export function defaultRandom(): number {\n\
             \x20 return Math.random(); // NOSONAR: jitter timing only, not a cryptographic use\n\
             }",
            Vec::new(),
        ),
        Decl::raw_providing(
            "retryDelay",
            "// retryDelay waits out one attempt's exponential-backoff delay before a\n\
             // retried call.\n\
             export async function retryDelay(attempt: number): Promise<void> {\n\
             \x20 await defaultSleep(backoffDelayMs(attempt, defaultRandom()));\n\
             }",
            Vec::new(),
        ),
    ]
}

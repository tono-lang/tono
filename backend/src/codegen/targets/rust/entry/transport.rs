//! The inline HTTP transport: per-operation Rust built directly from a
//! `WireBinding` (IR v8). Replaces the opaque `wire_descriptor` blob and the
//! `Runtime::execute` call into the hand-written `tono_http_runtime` crate:
//! the generated SDK now carries its own transport and imports no runtime
//! crate for it (`reqwest` stays as the native HTTP stack, behind the
//! consuming crate's default-on `reqwest` feature; the canonical transport
//! slot is the escape hatch when it is off). The shared declaration text the
//! emitted calls draw on lives in [`super::transport_decls`].
//!
//! Poda by use happens at two granularities: the shared declarations are
//! pruned SDK-wide by the usual root-group mechanism (an SDK with no
//! `@retry` anywhere drops the backoff helpers entirely), while the retry
//! loop and the timeout wrapping are inlined directly into `op_call`'s own
//! text, gated on `wire.retry`/`wire.timeout`, so a single operation with
//! neither carries no trace of either in its own generated method.

use crate::codegen::casing::CasingConfig;
use crate::codegen::entries::wire::{extra_success_codes, has_query, needs_record};
use crate::codegen::entries::EntryModel;
use crate::codegen::extensions::BoundExtension;
use crate::codegen::symbol::Symbol;
use crate::ir::{
    Module, Prim, TemplatePart, Tref, WireBinding, WirePart, WireResponsePart, WireValue,
};

use super::use_path;

/// Everything a settings-field read needs: the entry (for `@rename` and the
/// path's declared type) and the target casing. The read is typed (the
/// frontend resolves every ref position to a real field), so each spelling
/// renders by the leaf type instead of going through a runtime value bag.
pub(super) struct FieldCtx<'a> {
    pub entry: &'a EntryModel<'a>,
    pub module: &'a Module,
    pub config: &'a CasingConfig,
}

impl FieldCtx<'_> {
    /// The typed settings read for a field path, off `self.settings`.
    fn access(&self, path: &[String]) -> String {
        let mut out = String::from("self.settings.");
        for (i, seg) in path.iter().enumerate() {
            if i > 0 {
                out.push('.');
            }
            let rename = if i == 0 {
                self.entry.field_rename(seg, super::LANG)
            } else {
                None
            };
            out.push_str(&super::field_snake_ren(seg, rename.as_deref(), self.config));
        }
        out
    }

    fn leaf_type(&self, path: &[String]) -> Tref {
        self.entry.path_type(path, self.module)
    }

    /// An expression that `Display`s as the wire spelling of the field: every
    /// scalar the frontend allows in a ref position (string, numeric, bool,
    /// the branded well-knowns, an open enum) implements `Display` with
    /// exactly that spelling, so the access itself is enough.
    fn display_expr(&self, path: &[String]) -> String {
        self.access(path)
    }

    /// The field as an owned `String` (a header value position).
    fn string_expr(&self, path: &[String]) -> String {
        match self.leaf_type(path) {
            Tref::Prim(Prim::String | Prim::Uuid) => format!("{}.clone()", self.access(path)),
            _ => format!("{}.to_string()", self.access(path)),
        }
    }

    /// The field as a `&str` argument (a percent-encoding position): a
    /// string-shaped field borrows directly, everything else stringifies.
    fn str_ref_expr(&self, path: &[String]) -> String {
        match self.leaf_type(path) {
            Tref::Prim(Prim::String | Prim::Uuid) => format!("&{}", self.access(path)),
            Tref::Prim(Prim::Timestamp | Prim::Date | Prim::Duration) => {
                format!("&{}.0", self.access(path))
            }
            _ => format!("&{}.to_string()", self.access(path)),
        }
    }

    /// The field as an `i64` (`@retry`'s max): the frontend only accepts an
    /// integer-scalar field in this position, so the widening cast is exact.
    pub(super) fn i64_expr(&self, path: &[String]) -> String {
        format!("{} as i64", self.access(path))
    }
}

/// A Rust string literal for plain text (the `{s:?}` escaping rules cover
/// everything a wire binding's literals may carry).
fn rust_str(s: &str) -> String {
    format!("{s:?}")
}

/// Escape a literal run for use inside a `format!` template string.
fn fmt_lit(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '{' => out.push_str("{{"),
            '}' => out.push_str("}}"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

/// Render a parsed header-position template (a `request_headers` key, or a
/// `WireValue::Template`) into a Rust expression: a single literal run is a
/// plain `&str` literal, and a placeholder resolves either from the resolved
/// client settings (`Field`) or the call's own record (`Input`), both
/// unencoded (only a path position percent-encodes; see [`url_line`]).
fn template_expr(parts: &[TemplatePart], fields: &FieldCtx<'_>) -> String {
    if let [TemplatePart::Lit(s)] = parts {
        return rust_str(s);
    }
    if parts.is_empty() {
        return rust_str("");
    }
    let mut fmt = String::new();
    let mut args = Vec::new();
    for part in parts {
        match part {
            TemplatePart::Lit(s) => fmt.push_str(&fmt_lit(s)),
            TemplatePart::Field(path) => {
                fmt.push_str("{}");
                args.push(fields.display_expr(path));
            }
            TemplatePart::Input(name) => {
                fmt.push_str("{}");
                args.push(format!(
                    "format_scalar(record.get({}).unwrap_or(&serde_json::Value::Null))",
                    rust_str(name)
                ));
            }
        }
    }
    format!("format!(\"{fmt}\", {})", args.join(", "))
}

/// A `WireValue` position (a `request_headers` value) as an owned `String`.
fn wire_value_expr(v: &WireValue, fields: &FieldCtx<'_>) -> String {
    match v {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => format!("{}.to_string()", rust_str(s)),
            None => format!("{}.to_string()", rust_str(&json.to_string())),
        },
        WireValue::Field(path) => fields.string_expr(path),
        WireValue::Template(parts) => {
            let expr = template_expr(parts, fields);
            if expr.starts_with("format!") {
                expr
            } else {
                format!("{expr}.to_string()")
            }
        }
    }
}

/// The URL: the typed read of the resolved endpoint field concatenated with
/// the URI template. `validate_entries` re-checks at generation time that the
/// binding carries an endpoint, so the read needs no runtime guard.
fn url_line(wire: &WireBinding, has_query: bool, fields: &FieldCtx<'_>) -> String {
    let endpoint = wire
        .endpoint
        .as_ref()
        .expect("validate_entries rejects an entry @http op with no endpoint");
    let mut fmt = String::from("{}");
    let mut args = vec![fields.display_expr(endpoint)];
    for part in &wire.uri {
        match part {
            TemplatePart::Lit(s) => fmt.push_str(&fmt_lit(s)),
            TemplatePart::Field(path) => {
                fmt.push_str("{}");
                args.push(format!("percent_path({})", fields.str_ref_expr(path)));
            }
            TemplatePart::Input(name) => {
                fmt.push_str("{}");
                args.push(format!("path_part(record.get({}))", rust_str(name)));
            }
        }
    }
    let binding = if has_query { "let mut url" } else { "let url" };
    format!("{binding} = format!(\"{fmt}\", {});\n", args.join(", "))
}

/// One `set_header(...)` call per declared `request_headers` entry.
fn declared_header_lines(wire: &WireBinding, fields: &FieldCtx<'_>) -> String {
    wire.request_headers
        .iter()
        .map(|(key, value)| {
            let key_expr = template_expr(key, fields);
            let key_ref = if key_expr.starts_with('"') {
                key_expr
            } else {
                format!("&{key_expr}")
            };
            format!(
                "set_header(&mut headers, {key_ref}, {});\n",
                wire_value_expr(value, fields)
            )
        })
        .collect()
}

/// One guarded `set_header(...)` per input member bound to a header position:
/// an absent or null member sends no header, the same omission rule the
/// query and body positions follow.
fn per_call_header_lines(wire: &WireBinding) -> String {
    wire.bindings
        .iter()
        .filter_map(|(name, part)| match part {
            WirePart::Header { name: header_name } => Some(format!(
                "if let Some(v) = record.get({member}) {{\n    if !v.is_null() {{\n        set_header(&mut headers, {header}, format_scalar(v));\n    }}\n}}\n",
                member = rust_str(name),
                header = rust_str(header_name),
            )),
            _ => None,
        })
        .collect()
}

/// The query assembly: one `append_query(...)` per query-bound member, folded
/// into `url` only when anything landed.
fn query_lines(wire: &WireBinding) -> String {
    let mut out = String::from("let mut query: Vec<String> = Vec::new();\n");
    for (name, part) in &wire.bindings {
        if let WirePart::Query { name: query_name } = part {
            out.push_str(&format!(
                "append_query(&mut query, {}, record.get({}));\n",
                rust_str(query_name),
                rust_str(name)
            ));
        }
    }
    out.push_str(
        "if !query.is_empty() {\n    url.push('?');\n    url.push_str(&query.join(\"&\"));\n}\n",
    );
    out
}

/// The request body statement, or `None` when the operation sends no body. A
/// `Payload`-kind member wins outright; when every binding is a `Body` member
/// the typed input serializes directly (its serde impls already spell the
/// wire keys); a mixed binding re-collects the body members off the record.
fn body_lines(wire: &WireBinding, has_input: bool) -> Option<String> {
    if let Some((name, _)) = wire
        .bindings
        .iter()
        .find(|(_, p)| matches!(p, WirePart::Payload))
    {
        return Some(format!(
            "let body = record.get({}).map(|v| v.to_string());\n",
            rust_str(name)
        ));
    }
    let fields: Vec<&String> = wire
        .bindings
        .iter()
        .filter(|(_, p)| matches!(p, WirePart::Body))
        .map(|(name, _)| name)
        .collect();
    if fields.is_empty() || !has_input {
        return None;
    }
    if fields.len() == wire.bindings.len() {
        return Some(format!(
            "let body = Some(serde_json::to_string(&input).map_err(|e| {})?);\n",
            encode_failure("e")
        ));
    }
    let mut out = String::from("let mut body_members = serde_json::Map::new();\n");
    out.push_str(&format!(
        "for name in [{}] {{\n    if let Some(v) = record.get(name) {{\n        body_members.insert(name.to_string(), v.clone());\n    }}\n}}\n",
        fields
            .iter()
            .map(|f| rust_str(f))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("let body = Some(serde_json::Value::Object(body_members).to_string());\n");
    Some(out)
}

/// The Decode failure a failed input serialization maps to (the same category
/// and shape the descriptor-era record conversion used).
fn encode_failure(cause: &str) -> String {
    format!(
        "TonoError::Decode(DecodeError {{ path: \"$\".to_string(), expected: \"input\".to_string(), raw: {cause}.to_string() }})"
    )
}

/// `outcome.status >= 200 && ...`, plus one `||` arm per declared success
/// status outside the 2xx range (the shared [`extra_success_codes`] rule;
/// this only spells the Rust comparison).
fn success_expr(wire: &WireBinding) -> String {
    let mut out = String::from("outcome.status >= 200 && outcome.status < 300");
    for code in extra_success_codes(wire) {
        out.push_str(&format!(" || outcome.status == {code}"));
    }
    out
}

/// The lines folding the response-bound members (a header value, the status
/// code) into the success body, so the decoder sees them as ordinary fields.
/// Folding is a success-only concern (the same success test the
/// classification below runs, spelled against `response`): an error response
/// passes its body through untouched.
fn response_fold_lines(wire: &WireBinding) -> String {
    let mut sets = String::new();
    for (name, part) in &wire.response_bindings {
        let value = match part {
            WireResponsePart::StatusCode => "serde_json::Value::from(response.status)".to_string(),
            WireResponsePart::Header { name: header_name } => format!(
                "response.headers.get({}).map(|v| serde_json::Value::String(v.clone())).unwrap_or(serde_json::Value::Null)",
                rust_str(&header_name.to_lowercase())
            ),
        };
        sets.push_str(&format!(
            "    object.insert({}.to_string(), {value});\n",
            rust_str(name)
        ));
    }
    let condition = success_expr(wire).replace("outcome.status", "response.status");
    format!(
        "let outcome = if {condition} {{\n    let mut object = parse_json_object(&response.body);\n{sets}    HttpResponse {{ body: serde_json::Value::Object(object).to_string(), ..response }}\n}} else {{\n    response\n}};\n"
    )
}

/// The retry-or-fail tail shared by the transport-failure arm and the
/// declared-error check: retry while attempts remain (and, for a declared
/// error, while its `retryable()` read also holds), otherwise fail. With no
/// retry the tail collapses to the failure alone.
fn retry_or(has_retry: bool, extra_cond: Option<&str>, fail: &str) -> String {
    if !has_retry {
        return format!("{fail}\n");
    }
    let cond = match extra_cond {
        Some(c) => format!("attempt < max_retries && {c}"),
        None => "attempt < max_retries".to_string(),
    };
    format!(
        "if {cond} {{\n    (self.sleep)(backoff_delay_ms(attempt, (self.random)())).await;\n    attempt += 1;\n    continue;\n}}\n{fail}\n"
    )
}

/// One lifecycle hook invocation, or nothing when the slot is unbound: the
/// bespoke symbol is called directly (it is already an `async fn` over the
/// support shapes), and its failure classifies exactly like every other
/// bespoke boundary (a declared `TonoError` passes through, anything else
/// wraps into the Contract category under the slot's name).
fn hook_lines(
    slot: &str,
    var: &str,
    binding: Option<&BoundExtension<'_>>,
    refs: &mut Vec<Symbol>,
) -> String {
    let Some(b) = binding else {
        return String::new();
    };
    refs.push(Symbol::imported(b.symbol, use_path(b.module), b.symbol));
    format!(
        "let {var} = match {sym}({var}).await {{\n    Ok({var}) => {var},\n    Err(cause) => {{\n        return Err(match cause.downcast::<TonoError>() {{\n            Ok(declared) => *declared,\n            Err(other) => TonoError::Contract(ContractError {{ contract_name: {slot:?}.to_string(), cause: other }}),\n        }});\n    }}\n}};\n",
        sym = b.symbol,
    )
}

/// What [`op_call`] needs beyond the wire binding itself.
pub(super) struct OpCall<'a> {
    pub wire: &'a WireBinding,
    pub method: &'a str,
    pub has_input: bool,
    pub has_declared_errors: bool,
    /// The generated discriminator's name (read only with declared errors).
    pub discriminator: &'a str,
    /// The `Result` expression of the success path (built by `decode`), read
    /// against `outcome.body`.
    pub success_block: &'a str,
    pub before_request: Option<&'a BoundExtension<'a>>,
    pub after_response: Option<&'a BoundExtension<'a>>,
    /// The already-converted milliseconds field for the op's `@timeout` path.
    pub timeout_field: Option<String>,
}

/// One operation's transport body, replacing the descriptor-plus-`execute()`
/// call. Built once as a single "attempt" block and wrapped in a `loop` when
/// `wire.retry` is declared; a non-retrying operation runs the identical text
/// straight-line (its final failure sits in tail position, so the generated
/// method never ends on a bare `return`).
pub(super) fn op_call(call: &OpCall<'_>, fields: &FieldCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    let wire = call.wire;
    let has_retry = wire.retry.is_some();
    let query = has_query(wire);
    let body = body_lines(wire, call.has_input);

    refs.push(super::support_symbol("HttpRequest"));
    let mut out = String::new();
    let reads_input = needs_record(wire) || body.as_deref().is_some_and(|b| b.contains("&input"));
    if call.has_input && !reads_input {
        // An input no request position consumes (an empty struct bound to
        // nothing) is still part of the operation's declared signature; the
        // explicit discard keeps the generated method warning-clean.
        out.push_str("let _ = &input;\n");
    }
    if call.has_input && needs_record(wire) {
        out.push_str(&format!(
            "let record = serde_json::to_value(&input).map_err(|e| {})?;\n",
            encode_failure("e")
        ));
    }
    out.push_str(&url_line(wire, query, fields));
    if query {
        out.push_str(&query_lines(wire));
        refs.push(super::shared_symbol("append_query"));
    }
    out.push_str(
        "let mut headers: std::collections::HashMap<String, String> = std::collections::HashMap::new();\n",
    );
    refs.push(super::shared_symbol("set_header"));
    out.push_str(&declared_header_lines(wire, fields));
    out.push_str(
        "for (k, v) in &self.options.headers {\n    set_header(&mut headers, k, v.clone());\n}\n",
    );
    let per_call_headers = per_call_header_lines(wire);
    if !per_call_headers.is_empty() {
        refs.push(super::shared_symbol("format_scalar"));
        out.push_str(&per_call_headers);
    }
    if out.contains("path_part(") {
        refs.push(super::shared_symbol("path_part"));
    }
    if out.contains("percent_path(") {
        refs.push(super::shared_symbol("percent_path"));
    }
    let body_field = match &body {
        Some(lines) => {
            out.push_str(lines);
            let is_none_check = lines.starts_with("let body = record.get(");
            let guard = if is_none_check {
                "body.is_some() && "
            } else {
                ""
            };
            refs.push(super::shared_symbol("has_header"));
            out.push_str(&format!(
                "if {guard}!has_header(&headers, \"content-type\") {{\n    headers.insert(\"content-type\".to_string(), \"application/json\".to_string());\n}}\n",
            ));
            "body: body.clone()"
        }
        None => "body: None",
    };
    if call.timeout_field.is_some() {
        refs.push(super::shared_symbol("http_send_with_timeout"));
    } else {
        refs.push(super::shared_symbol("http_send"));
    }
    if has_retry {
        refs.push(super::shared_symbol("resolve_max_retries"));
        refs.push(super::shared_symbol("backoff_delay_ms"));
        let path = wire.retry.as_deref().unwrap_or_default();
        out.push_str(&format!(
            "let max_retries = resolve_max_retries({});\n",
            fields.i64_expr(path)
        ));
        out.push_str("let mut attempt: u32 = 0;\n");
    }

    let send_call = match &call.timeout_field {
        Some(field) => format!("http_send_with_timeout(&self.options, request, self.{field})"),
        None => "http_send(&self.options, request)".to_string(),
    };
    // A fresh headers copy per attempt: a before_request hook may rewrite the
    // map it receives, and a retried attempt must not see a prior attempt's
    // rewrite.
    let mut attempt = format!(
        "let request = HttpRequest {{ method: {method}.to_string(), url: url.clone(), headers: headers.clone(), {body_field} }};\n",
        method = rust_str(call.method),
    );
    attempt.push_str(&hook_lines(
        "before_request",
        "request",
        call.before_request,
        refs,
    ));
    let transport_fail = "Err(TonoError::Transport(TransportError { cause }))".to_string();
    let transport_arm = if has_retry {
        format!(
            "Err(cause) => {{\n{}}}\n",
            super::indent(
                &retry_or(true, None, &format!("return {transport_fail};")),
                1
            )
        )
    } else {
        format!("Err(cause) => return {transport_fail},\n")
    };
    let fold = !wire.response_bindings.is_empty();
    let needs_response_name = fold || call.after_response.is_some();
    let bind = if needs_response_name {
        "response"
    } else {
        "outcome"
    };
    attempt.push_str(&format!(
        "let {bind} = match {send_call}.await {{\n    Ok(response) => response,\n{arm}}};\n",
        arm = super::indent(&transport_arm, 1),
    ));
    attempt.push_str(&hook_lines(
        "after_response",
        "response",
        call.after_response,
        refs,
    ));
    if fold {
        refs.push(super::shared_symbol("parse_json_object"));
        refs.push(super::support_symbol("HttpResponse"));
        attempt.push_str(&response_fold_lines(wire));
    } else if needs_response_name {
        attempt.push_str("let outcome = response;\n");
    }
    // A one-expression success block returns bare; a multi-statement one
    // (a decode with its own `let`) keeps its block, which the braces lint
    // accepts exactly because it is not a lone expression.
    if call.success_block.contains('\n') {
        attempt.push_str(&format!(
            "if {} {{\n    return {{\n{}    }};\n}}\n",
            success_expr(wire),
            super::indent(call.success_block, 2),
        ));
    } else {
        attempt.push_str(&format!(
            "if {} {{\n    return {};\n}}\n",
            success_expr(wire),
            call.success_block,
        ));
    }
    if call.has_declared_errors {
        attempt.push_str(&format!(
            "let err = {}(outcome.status, &outcome.body);\n",
            call.discriminator
        ));
        let fail = if has_retry {
            "return Err(err);".to_string()
        } else {
            "Err(err)".to_string()
        };
        attempt.push_str(&retry_or(has_retry, Some("err.retryable()"), &fail));
    } else {
        let undeclared =
            "Err(TonoError::Api(APIFailure::Undeclared(APIError { status: outcome.status, body: outcome.body })))"
                .to_string();
        if has_retry {
            attempt.push_str(&format!("return {undeclared};\n"));
        } else {
            attempt.push_str(&format!("{undeclared}\n"));
        }
    }

    if has_retry {
        out.push_str(&format!("loop {{\n{}}}\n", super::indent(&attempt, 1)));
    } else {
        out.push_str(&attempt);
    }
    out
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

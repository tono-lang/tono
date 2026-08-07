//! One operation's transport call, built directly from its `WireBinding`:
//! assemble the URL, the layered headers, and the encoded body in the method's
//! own text, then drive the SDK's emitted `internal/transport` package (see
//! [`super::send`]) through one struct-request `Send`. Replaces the embedded
//! descriptor blob and the `Execute()` call into the hand-written HTTP
//! runtime.
//!
//! Poda by use is per operation: the `Timeout`, `Retry`, `Timing`, and `Hooks`
//! fields of the request literal appear only when the operation (or its
//! module) declares the piece, so a plain operation's method carries no trace
//! of any of them.

use std::collections::BTreeSet;

use crate::codegen::symbol::Symbol;
use crate::codegen::tree::symbol_slot;
use crate::ir::{TemplatePart, WireBinding, WirePart, WireResponsePart, WireValue};

use super::shared_symbol;

/// How a resolved entry-field read spells in a wire string position: a string
/// already, a branded string needing the `string(...)` flattening, or a value
/// the shared `FormatScalar` renders.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum FieldKind {
    StringLike,
    Branded,
    Other,
}

/// Everything one operation's call needs from its surroundings. The spellings
/// (`fail`, `field_access`, `field_kind`) stay closures so this module never
/// re-derives casing or hook routing.
pub(super) struct OpCall<'a> {
    pub wire: &'a WireBinding,
    pub has_input: bool,
    pub ret_zero: &'a str,
    /// The operation's error discriminator, when it declares errors.
    pub discriminator: Option<&'a str>,
    pub api_error: &'a str,
    pub transport_error: &'a str,
    /// The decode tail (returns or fails), at method depth.
    pub success_block: &'a str,
    /// Whether this module binds a request lifecycle hook, which is what puts
    /// a `hooks` field on the client.
    pub module_hooks: bool,
    /// The resolved `@retry` maximum as an `int` expression.
    pub retry_expr: Option<String>,
    /// The pre-converted `@timeout` read (a client field).
    pub timeout_expr: Option<String>,
}

/// The helper names the emitted text reached, so exactly those references are
/// declared (an unreached helper must stay prunable).
struct Reached(BTreeSet<&'static str>);

impl Reached {
    fn slot(&mut self, name: &'static str) -> String {
        self.0.insert(name);
        symbol_slot(name)
    }
}

/// A resolved field read in a scalar wire position (a header value, a template
/// run), flattened to a plain string expression.
fn scalar_expr(
    path: &[String],
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    reached: &mut Reached,
) -> String {
    let access = field_access(path);
    match field_kind(path) {
        FieldKind::StringLike => access,
        FieldKind::Branded => format!("string({access})"),
        FieldKind::Other => format!("{}({access})", reached.slot("FormatScalar")),
    }
}

/// A resolved field read handed to `PathPart` (which formats internally, so
/// only the branded flattening happens here).
fn path_part_arg(
    path: &[String],
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
) -> String {
    let access = field_access(path);
    match field_kind(path) {
        FieldKind::Branded => format!("string({access})"),
        _ => access,
    }
}

/// Render a parsed template into a Go string expression: a single literal run
/// is just a literal, and a placeholder resolves either from the resolved
/// client settings (`Field`) or the call's own record (`Input`). `escape` says
/// whether placeholders route through `PathPart` (URI positions) or
/// `FormatScalar` (header positions, which the wire does not percent-encode).
#[allow(clippy::too_many_arguments)]
fn template_expr(
    parts: &[TemplatePart],
    escape: bool,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    reached: &mut Reached,
) -> String {
    if let [TemplatePart::Lit(s)] = parts {
        return format!("{s:?}");
    }
    if parts.is_empty() {
        return "\"\"".to_string();
    }
    let mut out = Vec::new();
    for part in parts {
        match part {
            TemplatePart::Lit(s) => out.push(format!("{s:?}")),
            TemplatePart::Field(path) => {
                if escape {
                    let arg = path_part_arg(path, field_access, field_kind);
                    out.push(format!("{}({arg})", reached.slot("PathPart")));
                } else {
                    out.push(scalar_expr(path, field_access, field_kind, reached));
                }
            }
            TemplatePart::Input(name) => {
                let helper = if escape {
                    reached.slot("PathPartRaw")
                } else {
                    reached.slot("FormatRaw")
                };
                out.push(format!("{helper}(record[{name:?}])"));
            }
        }
    }
    out.join(" + ")
}

/// A `WireValue` position (a `request_headers` value) rendered as a string
/// expression: the two scalar forms, or a template.
fn wire_value_expr(
    value: &WireValue,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    reached: &mut Reached,
) -> String {
    match value {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => format!("{s:?}"),
            None => format!("{:?}", json.to_string()),
        },
        WireValue::Field(path) => scalar_expr(path, field_access, field_kind, reached),
        WireValue::Template(parts) => {
            template_expr(parts, false, field_access, field_kind, reached)
        }
    }
}

/// The base-URL read: the typed resolved endpoint field. The frontend rejects
/// an entry `@http` op that does not name a string endpoint field, and
/// `validate_entries` re-checks it at generation time (IR can arrive from a
/// file without ever passing the frontend), so by emission time the binding
/// always carries an endpoint and the read needs no runtime guard.
fn endpoint_expr(wire: &WireBinding, field_access: &dyn Fn(&[String]) -> String) -> String {
    let path = wire
        .endpoint
        .as_ref()
        .expect("validate_entries rejects an entry @http op with no endpoint");
    field_access(path)
}

fn has_query(wire: &WireBinding) -> bool {
    wire.bindings
        .values()
        .any(|p| matches!(p, WirePart::Query { .. }))
}

/// Whether the operation reads any input member individually via
/// `record[name]`: a URI placeholder, or any binding besides a plain `Body`
/// member (the all-body case marshals the typed input directly).
fn needs_record(wire: &WireBinding) -> bool {
    let uri_reads_record = wire
        .uri
        .iter()
        .any(|part| matches!(part, TemplatePart::Input(_)));
    uri_reads_record || wire.bindings.values().any(|p| !matches!(p, WirePart::Body))
}

/// `outcome.Status >= 200 && outcome.Status < 300`, plus one `||` arm per
/// declared success status outside the 2xx range: any 2xx succeeds even when
/// not literally declared.
fn success_expr(wire: &WireBinding) -> String {
    let mut out = String::from("outcome.Status >= 200 && outcome.Status < 300");
    for code in &wire.success {
        if !(200..300).contains(code) {
            out.push_str(&format!(" || outcome.Status == {code}"));
        }
    }
    out
}

/// The declared success statuses outside the 2xx range, for the retry loop's
/// own success classification (`Request.Success`).
fn extra_success_codes(wire: &WireBinding) -> Vec<i64> {
    wire.success
        .iter()
        .copied()
        .filter(|code| !(200..300).contains(code))
        .collect()
}

/// The URL assembly: the query entries only when a member is query-bound.
fn url_lines(
    wire: &WireBinding,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    reached: &mut Reached,
) -> String {
    let base = format!(
        "{} + {}",
        endpoint_expr(wire, field_access),
        template_expr(&wire.uri, true, field_access, field_kind, reached)
    );
    if !has_query(wire) {
        return format!("\trequestURL := {base}\n");
    }
    let mut out = String::from("\tvar query []string\n");
    let append = reached.slot("AppendQuery");
    for (name, part) in &wire.bindings {
        if let WirePart::Query { name: query_name } = part {
            out.push_str(&format!(
                "\tquery = {append}(query, {query_name:?}, record[{name:?}])\n"
            ));
        }
    }
    out.push_str(&format!(
        "\trequestURL := {base} + {}(query)\n",
        reached.slot("QueryString")
    ));
    out
}

/// The header assembly, layered the way the runtime layered them: the declared
/// headers first, the caller's base headers over them, and the input's header
/// bindings last (the per-call value is the most specific). An absent per-call
/// member omits its header rather than sending an empty value.
fn header_lines(
    wire: &WireBinding,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    reached: &mut Reached,
) -> String {
    let set = reached.slot("SetHeader");
    let mut out = String::from("\theaders := map[string]string{}\n");
    for (key, value) in &wire.request_headers {
        out.push_str(&format!(
            "\t{set}(headers, {}, {})\n",
            template_expr(key, false, field_access, field_kind, reached),
            wire_value_expr(value, field_access, field_kind, reached),
        ));
    }
    out.push_str(&format!(
        "\tfor name, value := range c.settings.Headers {{\n\t\t{set}(headers, name, value)\n\t}}\n"
    ));
    for (name, part) in &wire.bindings {
        if let WirePart::Header { name: header_name } = part {
            let format = reached.slot("FormatRaw");
            out.push_str(&format!(
                "\tif v, ok := record[{name:?}]; ok && string(v) != \"null\" {{\n\
                 \t\t{set}(headers, {header_name:?}, {format}(v))\n\
                 \t}}\n"
            ));
        }
    }
    out
}

/// The request body: a `Payload`-kind member wins outright; every binding
/// being a plain `Body` member marshals the typed input directly (the encoded
/// input already *is* exactly the body, correct even under `@wire` renames);
/// a mixed set assembles just the `Body` members. `None` when the operation
/// sends no body at all. Returns the lines and whether the content-type
/// default needs the runtime `body != nil` guard (a statically-known body
/// does not).
fn body_lines(
    call: &OpCall<'_>,
    fail: &dyn Fn(String) -> String,
    reached: &mut Reached,
) -> (String, Option<bool>) {
    let wire = call.wire;
    let ret_zero = call.ret_zero;
    if let Some((name, _)) = wire
        .bindings
        .iter()
        .find(|(_, p)| matches!(p, WirePart::Payload))
    {
        // The record already holds the member's raw JSON bytes, so the whole
        // body is a slice, not a re-encode.
        let text = format!(
            "\tvar body []byte\n\
             \tif v, ok := record[{name:?}]; ok {{\n\
             \t\tbody = v\n\
             \t}}\n"
        );
        return (text, Some(true));
    }
    let body_members: Vec<&String> = wire
        .bindings
        .iter()
        .filter(|(_, p)| matches!(p, WirePart::Body))
        .map(|(name, _)| name)
        .collect();
    if body_members.is_empty() || !call.has_input {
        return (String::new(), None);
    }
    if body_members.len() == wire.bindings.len() {
        let text = format!(
            "\tbody, err := json.Marshal(input)\n\
             \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
            fail_enc = fail("err".to_string()),
        );
        return (text, Some(false));
    }
    let members = body_members
        .iter()
        .map(|name| format!("{name:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let text = format!(
        "\tbody := {encode}(record, {members})\n",
        encode = reached.slot("EncodeBody"),
    );
    // EncodeBody yields nil when every member is absent, so the default
    // content-type keeps the runtime's body-presence guard.
    (text, Some(true))
}

/// The `Retry` field value: the resolved maximum, plus the `When` predicate
/// built from the operation's own discriminator, so the retry decision and the
/// decoded error type can never disagree (a declared error's own `Retryable()`
/// is the only place `@retryable` is materialized). An op with no declared
/// errors carries no predicate: no error response is ever retryable.
fn retry_field(call: &OpCall<'_>, reached: &mut Reached) -> Option<String> {
    let max = call.retry_expr.as_ref()?;
    let retry = reached.slot("Retry");
    match call.discriminator {
        Some(discriminator) => Some(format!(
            "{retry}{{Max: {max}, When: func(status int, body string) bool {{\n\
             \t\t\tif re, ok := {discriminator}(status, []byte(body)).(interface{{ Retryable() bool }}); ok {{\n\
             \t\t\t\treturn re.Retryable()\n\
             \t\t\t}}\n\
             \t\t\treturn false\n\
             \t\t}}}}"
        )),
        None => Some(format!("{retry}{{Max: {max}}}")),
    }
}

/// The `map[string]json.RawMessage` literal folding the response-bound
/// members in: raw JSON so folding a status or header into the response body
/// never rewrites one of the body's own numbers.
fn fold_map_expr(wire: &WireBinding, reached: &mut Reached) -> String {
    let entries = wire
        .response_bindings
        .iter()
        .map(|(member, part)| match part {
            WireResponsePart::StatusCode => {
                format!("{member:?}: json.RawMessage(strconv.Itoa(outcome.Status))")
            }
            WireResponsePart::Header { name } => format!(
                "{member:?}: {}(outcome.Headers, {:?})",
                reached.slot("HeaderValue"),
                name.to_lowercase()
            ),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("map[string]json.RawMessage{{{entries}}}")
}

/// One operation's transport call: everything between the input validation and
/// the method's closing brace.
pub(super) fn op_call(
    call: &OpCall<'_>,
    fail: &dyn Fn(String) -> String,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    refs: &mut Vec<Symbol>,
) -> String {
    let wire = call.wire;
    let ret_zero = call.ret_zero;
    let mut reached = Reached(BTreeSet::new());
    let mut out = String::new();

    let with_record = call.has_input && needs_record(wire);
    if with_record {
        refs.push(shared_symbol("EncodeRecord"));
        out.push_str(&format!(
            "\trecord, err := {encode}(input)\n\
             \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
            encode = symbol_slot("EncodeRecord"),
            fail_enc = fail("err".to_string()),
        ));
    }
    out.push_str(&url_lines(wire, field_access, field_kind, &mut reached));
    out.push_str(&header_lines(wire, field_access, field_kind, &mut reached));
    let (body_text, content_type_guard) = body_lines(call, fail, &mut reached);
    out.push_str(&body_text);
    if body_text.contains("json.Marshal") {
        refs.push(super::import("json", "encoding/json"));
    }
    if let Some(guarded) = content_type_guard {
        let has = reached.slot("HasHeader");
        let guard = if guarded { "body != nil && " } else { "" };
        out.push_str(&format!(
            "\tif {guard}!{has}(headers, \"content-type\") {{\n\
             \t\theaders[\"content-type\"] = \"application/json\"\n\
             \t}}\n"
        ));
    }

    // The request literal: the call itself always, the policy fields only when
    // the operation (or its module) declares the policy.
    let mut fields = vec![
        ("Method", format!("{:?}", wire.method)),
        ("URL", "requestURL".to_string()),
        ("Headers", "headers".to_string()),
    ];
    if content_type_guard.is_some() {
        fields.push(("Body", "body".to_string()));
    }
    if let Some(timeout) = &call.timeout_expr {
        fields.push(("Timeout", timeout.clone()));
    }
    let extra_success = extra_success_codes(wire);
    if call.retry_expr.is_some() && !extra_success.is_empty() {
        let list = extra_success
            .iter()
            .map(|code| code.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        fields.push(("Success", format!("[]int{{{list}}}")));
    }
    if call.retry_expr.is_some() {
        fields.push(("Timing", "c.timing".to_string()));
    }
    if call.module_hooks {
        fields.push(("Hooks", "c.hooks".to_string()));
    }
    if let Some(retry) = retry_field(call, &mut reached) {
        fields.push(("Retry", retry));
    }
    let field_lines: String = fields
        .iter()
        .map(|(name, value)| format!("\t\t{name}: {value},\n"))
        .collect();
    // A hook's own failure propagates raw; the check exists only where a hook
    // is bound, so a hook-free module's methods carry no dead error channel.
    let hook_check = if call.module_hooks {
        format!(
            "\tif outcome.HookErr != nil {{\n\t\treturn {ret_zero}{fail_hook}\n\t}}\n",
            fail_hook = fail("outcome.HookErr".to_string()),
        )
    } else {
        String::new()
    };
    out.push_str(&format!(
        "\toutcome := {send}(ctx, c.settings.HTTPClient, c.settings.Transport, {request}{{\n\
         {field_lines}\
         \t}})\n\
         {hook_check}\
         \tif outcome.Cause != nil {{\n\t\treturn {ret_zero}{fail_transport}\n\t}}\n",
        send = reached.slot("Send"),
        request = reached.slot("Request"),
        fail_transport = fail(format!(
            "&{transport}{{Cause: outcome.Cause}}",
            transport = call.transport_error
        )),
    ));

    out.push_str(&format!("\tif {} {{\n", success_expr(wire)));
    if !wire.response_bindings.is_empty() {
        out.push_str(&format!(
            "\t\tfolded := {}(outcome.Body, {})\n",
            reached.slot("FoldResponse"),
            fold_map_expr(wire, &mut reached),
        ));
        // fold_map_expr spells json.RawMessage (and strconv.Itoa for a
        // statusCode binding) directly into this method's own text.
        refs.push(super::import("json", "encoding/json"));
        if wire
            .response_bindings
            .values()
            .any(|p| matches!(p, WireResponsePart::StatusCode))
        {
            refs.push(super::import("strconv", "strconv"));
        }
    }
    out.push_str(&indent(call.success_block));
    out.push_str("\t}\n");
    let error_expr = match call.discriminator {
        Some(discriminator) => format!("{discriminator}(outcome.Status, []byte(outcome.Body))"),
        None => format!(
            "&{api}{{Status: outcome.Status, Body: outcome.Body}}",
            api = call.api_error
        ),
    };
    out.push_str(&format!("\treturn {ret_zero}{}\n", fail(error_expr)));

    for name in reached.0 {
        refs.push(shared_symbol(name));
    }
    out
}

/// One more tab on every non-empty line, for the success tail moving inside
/// the status check.
fn indent(block: &str) -> String {
    block
        .lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_string()
            } else {
                format!("\t{line}\n")
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;

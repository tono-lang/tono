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

use crate::codegen::casing::CasingConfig;
use crate::codegen::entries::plan::push_gap;
use crate::codegen::entries::wire::{
    body_reads_record, has_query, needs_record_for_reads, success_test_expr,
};
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::symbol_slot;
use crate::ir::{Module, TemplatePart, WireBinding, WireResponsePart, WireValue};

use super::resolve_wire_call::{call_body_stmt, call_header_lines};
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

/// A resolved param-member access off `input`: the Go field identifier
/// (`@rename(go)` already applied) and its [`FieldKind`]. `None` when the
/// caller cannot resolve the member through typed field access (a
/// cross-module parameter type, most commonly), in which case the position
/// falls back to indexing the decoded record.
pub(super) type ParamAccess<'a> = &'a dyn Fn(&str) -> Option<(String, FieldKind)>;

/// Everything one operation's call needs from its surroundings. The spellings
/// (`fail`, `field_access`, `field_kind`) stay closures so this module never
/// re-derives casing or hook routing.
pub(super) struct OpCall<'a> {
    pub wire: &'a WireBinding,
    pub module: &'a Module,
    pub config: &'a CasingConfig,
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
pub(super) struct Reached(BTreeSet<&'static str>);

impl Reached {
    pub(super) fn slot(&mut self, name: &'static str) -> String {
        self.0.insert(name);
        symbol_slot(name)
    }
}

/// The wire-format rendering rules for an already-resolved (access
/// expression, kind) pair: shared by an entry-sibling field path and a
/// resolved param member, which render identically once each has produced its
/// own typed access expression.
fn scalar_of(access: &str, kind: FieldKind, reached: &mut Reached) -> String {
    match kind {
        FieldKind::StringLike => access.to_string(),
        FieldKind::Branded => format!("string({access})"),
        FieldKind::Other => format!("{}({access})", reached.slot("FormatScalar")),
    }
}

/// The `PathPart`-bound form (which formats internally, so only the branded
/// flattening happens here) of the same resolved access.
fn path_part_of(access: &str, kind: FieldKind) -> String {
    match kind {
        FieldKind::Branded => format!("string({access})"),
        _ => access.to_string(),
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
    scalar_of(&field_access(path), field_kind(path), reached)
}

/// A resolved field read handed to `PathPart` (which formats internally, so
/// only the branded flattening happens here).
fn path_part_arg(
    path: &[String],
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
) -> String {
    path_part_of(&field_access(path), field_kind(path))
}

/// A `Param(segs)`/`TemplatePart::Param` position in a scalar wire string:
/// the whole parameter (`segs` empty) always routes through the shared
/// helper (its shape is not statically known here), a resolved member routes
/// through the same typed rendering a `Field` path gets (bypassing the helper
/// where the kind allows it), and an unresolved member falls back to the
/// helper over the decoded record, exactly as before this position could ever
/// resolve. `escape` picks `PathPart` (URI positions) or `FormatScalar`
/// (header positions).
fn param_scalar_expr(
    segs: &[String],
    param_access: ParamAccess<'_>,
    escape: bool,
    reached: &mut Reached,
) -> String {
    match segs.first() {
        None => {
            let helper = if escape {
                reached.slot("PathPart")
            } else {
                reached.slot("FormatScalar")
            };
            format!("{helper}(input)")
        }
        Some(name) => match param_access(name) {
            Some((field, kind)) => {
                let access = format!("input.{field}");
                if escape {
                    format!(
                        "{}({})",
                        reached.slot("PathPart"),
                        path_part_of(&access, kind)
                    )
                } else {
                    scalar_of(&access, kind, reached)
                }
            }
            None => {
                let helper = if escape {
                    reached.slot("PathPart")
                } else {
                    reached.slot("FormatScalar")
                };
                format!("{helper}(record[{name:?}])")
            }
        },
    }
}

/// A `Param(segs)` position rendered as a Go `any` expression (a query value
/// or a @body ctor field): the same branding `Field` gets in
/// [`wire_value_any_expr`], applied to a resolved member's typed access, or
/// the decoded record when unresolved.
fn param_any_expr(segs: &[String], param_access: ParamAccess<'_>) -> String {
    match segs.first() {
        None => "input".to_string(),
        Some(name) => match param_access(name) {
            Some((field, kind)) => path_part_of(&format!("input.{field}"), kind),
            None => format!("record[{name:?}]"),
        },
    }
}

/// Render a parsed template into a Go string expression: a single literal run
/// is just a literal, and a placeholder resolves either from the resolved
/// client settings (`Field`) or the call's own record (`Input`). `escape` says
/// whether placeholders route through `PathPart` (URI positions) or
/// `FormatScalar` (header positions, which the wire does not percent-encode).
#[allow(clippy::too_many_arguments)]
pub(super) fn template_expr(
    parts: &[TemplatePart],
    escape: bool,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
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
                    reached.slot("PathPart")
                } else {
                    reached.slot("FormatScalar")
                };
                out.push(format!("{helper}(record[{name:?}])"));
            }
            // [] is the whole typed parameter (read as `input` directly, no
            // record needed); one segment is a member of the input struct,
            // resolved through typed field access when the target can (see
            // [`param_scalar_expr`]), the legacy decoded-record read
            // otherwise (an op has exactly one parameter, so "member of the
            // parameter" and "member of the input" are the same decoded
            // record). Deeper paths are not reachable: the typechecker only
            // resolves an op-parameter reference one level deep.
            TemplatePart::Param(segs) => {
                out.push(param_scalar_expr(segs, param_access, escape, reached));
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
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    match value {
        WireValue::Lit(json) => match json.as_str() {
            Some(s) => format!("{s:?}"),
            None => format!("{:?}", json.to_string()),
        },
        WireValue::Field(path) => scalar_expr(path, field_access, field_kind, reached),
        WireValue::Param(segs) => param_scalar_expr(segs, param_access, false, reached),
        WireValue::Template(parts) => template_expr(
            parts,
            false,
            field_access,
            field_kind,
            param_access,
            reached,
        ),
        // The frontend only ever emits Object for @body, read through
        // wire_value_any_expr + json.Marshal (see body_lines), never through
        // this string-position renderer.
        WireValue::Object(_) => unreachable!("a wire object never reaches a scalar position"),
        // A top-level header call is rendered as its own statement by
        // `call_header_lines` (`header_lines` filters it out before reaching
        // here); Go's error handling has no expression-position form for a
        // fallible call (unlike Rust's inline `match`), so a call can only
        // ever be a statement, never nested inside a scalar-position
        // expression.
        WireValue::Call(_) => {
            unreachable!("a header position never carries a top-level extern call here")
        }
    }
}

/// A Go literal for a JSON scalar; a compound literal (object/array) is
/// unreachable here, since it would only arise nested inside a @body ctor's
/// field value, and the frontend's ctor typecheck only accepts a reference
/// or a scalar literal/template per field (RFC-0022 §4: zero new expressive
/// power).
pub(super) fn go_json_lit(json: &serde_json::Value) -> String {
    match json {
        serde_json::Value::Null => "nil".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            unreachable!("a @body ctor field value is a reference or a scalar literal")
        }
    }
}

/// A `WireValue` rendered as a Go `any` expression: the @body ctor mapper's
/// field value, or nested inside one. Unlike [`wire_value_expr`] (a string),
/// this keeps the value's native shape so `json.Marshal` encodes it the same
/// way it would encode the field directly.
fn wire_value_any_expr(
    value: &WireValue,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    match value {
        WireValue::Lit(json) => go_json_lit(json),
        WireValue::Field(path) => {
            let access = field_access(path);
            match field_kind(path) {
                FieldKind::Branded => format!("string({access})"),
                _ => access,
            }
        }
        WireValue::Param(segs) => param_any_expr(segs, param_access),
        WireValue::Template(parts) => template_expr(
            parts,
            false,
            field_access,
            field_kind,
            param_access,
            reached,
        ),
        WireValue::Object(entries) => {
            let items: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{k:?}: {}",
                        wire_value_any_expr(v, field_access, field_kind, param_access, reached)
                    )
                })
                .collect();
            format!("map[string]any{{{}}}", items.join(", "))
        }
        // A top-level @body call is rendered as its own statement (see
        // `call_body_stmt`; `body_lines` skips it here). A call nested
        // inside a ctor field would need the same statement-hoisting Go's
        // fallible-call error handling has no expression form for;
        // `validate::wire_call_resolves` rejects that shape upstream, so it
        // never reaches this renderer.
        WireValue::Call(_) => unreachable!(
            "validate::wire_call_resolves rejects an extern call nested inside a ctor field"
        ),
    }
}

/// A `path`/`uri` position: a literal or pure reference passes through
/// unescaped (the author took responsibility for its content by not writing
/// a template), while a template's placeholders are escaped as one path
/// segment each (`escape: true`).
fn uri_expr(
    value: &WireValue,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    match value {
        WireValue::Template(parts) => {
            template_expr(parts, true, field_access, field_kind, param_access, reached)
        }
        other => wire_value_expr(other, field_access, field_kind, param_access, reached),
    }
}

/// The base-URL read: the resolved endpoint value (an entry field in
/// practice; the grammar also allows a literal, a template, or an
/// op-parameter reference). The frontend rejects an entry `@http` op that
/// does not name an endpoint, and `validate_entries` re-checks it at
/// generation time (IR can arrive from a file without ever passing the
/// frontend), so by emission time the binding always carries one and the
/// read needs no runtime guard.
fn endpoint_expr(
    wire: &WireBinding,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    let value = wire
        .endpoint
        .as_ref()
        .expect("validate_entries rejects an entry @http op with no endpoint");
    wire_value_expr(value, field_access, field_kind, param_access, reached)
}

/// The Go spelling of the shared [`success_test_expr`] rule.
fn success_expr(wire: &WireBinding) -> String {
    success_test_expr(wire, "outcome.Status", "==")
}

/// The URL assembly: the query entries only when the operation declares a
/// `@query`.
fn url_lines(
    wire: &WireBinding,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    let base = format!(
        "{} + {}",
        endpoint_expr(wire, field_access, field_kind, param_access, reached),
        uri_expr(&wire.uri, field_access, field_kind, param_access, reached)
    );
    if !has_query(wire) {
        return format!("\trequestURL := {base}\n");
    }
    let mut out = String::from("\tvar query []string\n");
    let append = reached.slot("AppendQuery");
    for (key, value) in &wire.query {
        out.push_str(&format!(
            "\tquery = {append}(query, {}, {})\n",
            template_expr(key, false, field_access, field_kind, param_access, reached),
            wire_value_any_expr(value, field_access, field_kind, param_access, reached),
        ));
    }
    out.push_str(&format!(
        "\trequestURL := {base} + {}(query)\n",
        reached.slot("QueryString")
    ));
    out
}

/// The header assembly, layered the way the runtime layered them: the
/// declared headers first, then the caller's base headers over them (the
/// per-call value is the most specific).
fn header_lines(
    wire: &WireBinding,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    reached: &mut Reached,
) -> String {
    let set = reached.slot("SetHeader");
    let mut out = String::from("\theaders := map[string]string{}\n");
    for (key, value) in &wire.request_headers {
        // A call-valued header reads `.request`, which does not exist yet
        // at this point in assembly; `call_header_lines` patches it in once
        // the request is fully built.
        if matches!(value, WireValue::Call(_)) {
            continue;
        }
        out.push_str(&format!(
            "\t{set}(headers, {}, {})\n",
            template_expr(key, false, field_access, field_kind, param_access, reached),
            wire_value_expr(value, field_access, field_kind, param_access, reached),
        ));
    }
    out.push_str(&format!(
        "\tfor name, value := range c.settings.Headers {{\n\t\t{set}(headers, name, value)\n\t}}\n"
    ));
    out
}

/// The request body, or `None` when the operation sends no body: `wire.body`
/// says exactly what the body is, never inferred from what the input leaves
/// undeclared. The whole-parameter form marshals the typed input directly
/// (correct even under `@wire` renames); a single member marshals it raw off
/// the record; every other form (an entry-field reference, a template, or
/// the @body ctor mapper) builds the value explicitly before marshaling it.
/// Returns the lines and whether the content-type default needs the runtime
/// `body != nil` guard (a statically-known body does not).
fn body_lines(
    call: &OpCall<'_>,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    fail: &dyn Fn(String) -> String,
    reached: &mut Reached,
) -> (String, Option<bool>) {
    let wire = call.wire;
    let ret_zero = call.ret_zero;
    let Some(body) = wire.body.as_ref() else {
        return (String::new(), None);
    };
    // A call-valued body reads `.request`, which does not exist yet at this
    // point in assembly; `call_body_stmt` patches it in once the request is
    // fully built (the "None" content-type guard skips the normal `body`
    // field here, exactly like the no-body case).
    if matches!(body, WireValue::Call(_)) {
        return (String::new(), None);
    }
    if let WireValue::Param(segs) = body {
        if segs.is_empty() {
            let text = format!(
                "\tbody, err := json.Marshal(input)\n\
                 \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
                fail_enc = fail("err".to_string()),
            );
            return (text, Some(false));
        }
        let name = segs
            .first()
            .expect("a @body param reference resolves zero or one segment deep");
        let text = format!(
            "\tvar body []byte\n\
             \tif v, ok := record[{name:?}]; ok {{\n\
             \t\tencoded, err := json.Marshal(v)\n\
             \t\tif err != nil {{\n\t\t\treturn {ret_zero}{fail_enc}\n\t\t}}\n\
             \t\tbody = encoded\n\
             \t}}\n",
            fail_enc = fail("err".to_string()),
        );
        return (text, Some(true));
    }
    let text = format!(
        "\tbody, err := json.Marshal({})\n\
         \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
        wire_value_any_expr(body, field_access, field_kind, param_access, reached),
        fail_enc = fail("err".to_string()),
    );
    (text, Some(false))
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

/// The `map[string]any` literal folding the response-bound members in.
fn fold_map_expr(wire: &WireBinding, reached: &mut Reached) -> String {
    let entries = wire
        .response_bindings
        .iter()
        .map(|(member, part)| match part {
            WireResponsePart::StatusCode => format!("{member:?}: outcome.Status"),
            WireResponsePart::Header { name } => format!(
                "{member:?}: {}(outcome.Headers, {:?})",
                reached.slot("HeaderValue"),
                name.to_lowercase()
            ),
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("map[string]any{{{entries}}}")
}

/// One operation's transport call: everything between the input validation and
/// the method's closing brace.
pub(super) fn op_call(
    call: &OpCall<'_>,
    fail: &dyn Fn(String) -> String,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    refs: &mut Vec<Symbol>,
) -> String {
    let wire = call.wire;
    let ret_zero = call.ret_zero;
    let mut reached = Reached(BTreeSet::new());
    let mut out = String::new();

    let resolves = |name: &str| param_access(name).is_some();
    let with_record = call.has_input
        && (needs_record_for_reads(wire, &resolves) || body_reads_record(wire, &resolves));
    if with_record {
        refs.push(shared_symbol("EncodeRecord"));
        out.push_str(&format!(
            "\trecord, err := {encode}(input)\n\
             \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
            encode = symbol_slot("EncodeRecord"),
            fail_enc = fail("err".to_string()),
        ));
    }
    push_gap(&mut out);
    out.push_str(&url_lines(
        wire,
        field_access,
        field_kind,
        param_access,
        &mut reached,
    ));
    push_gap(&mut out);
    out.push_str(&header_lines(
        wire,
        field_access,
        field_kind,
        param_access,
        &mut reached,
    ));
    let (body_text, content_type_guard) = body_lines(
        call,
        field_access,
        field_kind,
        param_access,
        fail,
        &mut reached,
    );
    if !body_text.is_empty() {
        push_gap(&mut out);
        out.push_str(&body_text);
    }
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
    if call.retry_expr.is_some() && !wire.success.is_empty() {
        let list = wire
            .success
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
    let request = reached.slot("Request");
    // A call-valued header/body reads the request once it exists: it patches
    // in right here, materialized as its own variable so the call has
    // something to read, right before it is sent.
    let mut call_lines = call_header_lines(
        wire,
        call.module,
        call.config,
        field_access,
        field_kind,
        param_access,
        "req",
        ret_zero,
        fail,
        &mut reached,
        refs,
    );
    call_lines.push_str(&call_body_stmt(
        wire,
        call.module,
        call.config,
        field_access,
        param_access,
        "req",
        ret_zero,
        fail,
        refs,
    ));
    push_gap(&mut out);
    let send_arg = if call_lines.is_empty() {
        format!("{request}{{\n{field_lines}\t}}")
    } else {
        out.push_str(&format!("\treq := {request}{{\n{field_lines}\t}}\n"));
        out.push_str(&call_lines);
        "req".to_string()
    };
    out.push_str(&format!(
        "\toutcome := {send}(ctx, c.settings.HTTPClient, c.settings.Transport, {send_arg})\n\
         {hook_check}\
         \tif outcome.Cause != nil {{\n\t\treturn {ret_zero}{fail_transport}\n\t}}\n",
        send = reached.slot("Send"),
        fail_transport = fail(format!(
            "&{transport}{{Cause: outcome.Cause}}",
            transport = call.transport_error
        )),
    ));

    push_gap(&mut out);
    out.push_str(&format!("\tif {} {{\n", success_expr(wire)));
    if !wire.response_bindings.is_empty() {
        out.push_str(&format!(
            "\t\tfolded := {}(outcome.Body, {})\n",
            reached.slot("FoldResponse"),
            fold_map_expr(wire, &mut reached),
        ));
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
    push_gap(&mut out);
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

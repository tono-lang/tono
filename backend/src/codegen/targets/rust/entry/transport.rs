//! The inline HTTP transport: per-operation Rust built directly from a
//! `WireBinding`. The generated SDK carries its own transport and imports no
//! runtime crate for it (`reqwest` stays as the native HTTP stack, behind the
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
use crate::codegen::conventions::field_ident;
use crate::codegen::entries::plan::push_gap;
use crate::codegen::entries::wire::{
    body_reads_record, has_query, needs_record, needs_record_for_reads, success_test_expr,
};
use crate::codegen::entries::EntryModel;
use crate::codegen::symbol::Symbol;
use crate::ir::{Module, Prim, TemplatePart, Tref, WireBinding, WireResponsePart, WireValue};

use super::resolve_wire_call::{call_body_stmt, call_header_lines};

/// Everything a settings-field read needs: the entry (for `@rename` and the
/// path's declared type) and the target casing. The read is typed (the
/// frontend resolves every ref position to a real field), so each spelling
/// renders by the leaf type instead of going through a runtime value bag.
pub(super) struct FieldCtx<'a> {
    pub entry: &'a EntryModel<'a>,
    pub module: &'a Module,
    pub config: &'a CasingConfig,
    /// The op's own declared parameter type, for resolving a `Param(segs)`
    /// member through typed field access (see [`FieldCtx::param`]).
    pub input: Option<&'a Tref>,
}

impl FieldCtx<'_> {
    /// The typed settings read for a field path, off `self.settings`.
    pub(super) fn access(&self, path: &[String]) -> String {
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

    /// A `Param(segs)` member resolved through typed field access: the op's
    /// own parameter type, chased to a same-module structure and its one
    /// member matching `seg`. `None` when the parameter type is not a
    /// same-module structure (a cross-module reference) or has no such
    /// member; the caller falls back to the decoded record in that case.
    pub(super) fn param(&self, seg: &str) -> Option<(String, Tref)> {
        let member = crate::codegen::ops::param_member(self.module, self.input, seg)?;
        Some((
            format!("input.{}", field_ident(member, self.config, super::LANG)),
            member.target.clone(),
        ))
    }

    /// The `Display`-typed form of a resolved param member.
    fn param_display_expr(&self, seg: &str) -> Option<String> {
        self.param(seg).map(|(access, _)| access)
    }

    /// The resolved param member as an owned `String`, mirroring
    /// [`FieldCtx::string_expr`].
    fn param_string_expr(&self, seg: &str) -> Option<String> {
        self.param(seg).map(|(access, ty)| match ty {
            Tref::Prim(Prim::String | Prim::Uuid) => format!("{access}.clone()"),
            _ => format!("{access}.to_string()"),
        })
    }

    /// The resolved param member as a `&str` argument, mirroring
    /// [`FieldCtx::str_ref_expr`].
    fn param_str_ref_expr(&self, seg: &str) -> Option<String> {
        self.param(seg).map(|(access, ty)| match ty {
            Tref::Prim(Prim::String | Prim::Uuid) => format!("&{access}"),
            Tref::Prim(Prim::Timestamp | Prim::Date | Prim::Duration) => format!("&{access}.0"),
            _ => format!("&{access}.to_string()"),
        })
    }

    /// The resolved param member as a `serde_json::Value`-producing
    /// expression, mirroring the `Field` arm of [`wire_value_json_expr`].
    fn param_json_expr(&self, seg: &str) -> Option<String> {
        self.param(seg).map(|(access, _)| {
            format!("serde_json::to_value(&{access}).unwrap_or(serde_json::Value::Null)")
        })
    }
}

/// A Rust string literal for plain text (the `{s:?}` escaping rules cover
/// everything a wire binding's literals may carry).
pub(super) fn rust_str(s: &str) -> String {
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
pub(super) fn template_expr(parts: &[TemplatePart], fields: &FieldCtx<'_>) -> String {
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
            // [] is the whole typed parameter (read as `input` directly, no
            // record needed); one segment is a member of the input struct,
            // resolved through typed field access when the target can (see
            // [`FieldCtx::param_display_expr`]), the legacy decoded-record
            // read otherwise (an op has exactly one parameter, so "member of
            // the parameter" and "member of the input" are the same decoded
            // record). Deeper paths are not reachable: the typechecker only
            // resolves an op-parameter reference one level deep.
            TemplatePart::Param(segs) => {
                fmt.push_str("{}");
                args.push(match segs.first() {
                    None => "input.to_string()".to_string(),
                    Some(name) => fields.param_display_expr(name).unwrap_or_else(|| {
                        format!(
                            "format_scalar(record.get({}).unwrap_or(&serde_json::Value::Null))",
                            rust_str(name)
                        )
                    }),
                });
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
        WireValue::Param(segs) => match segs.first() {
            None => "input.to_string()".to_string(),
            Some(name) => fields.param_string_expr(name).unwrap_or_else(|| {
                format!(
                    "format_scalar(record.get({}).unwrap_or(&serde_json::Value::Null)).to_string()",
                    rust_str(name)
                )
            }),
        },
        WireValue::Template(parts) => {
            let expr = template_expr(parts, fields);
            if expr.starts_with("format!") {
                expr
            } else {
                format!("{expr}.to_string()")
            }
        }
        // Only ever emitted for @body; never a header/query/uri value.
        WireValue::Object(_) => format!("{}.to_string()", wire_value_json_expr(v, fields)),
        // A top-level header/@body call is rendered directly by
        // `call_wire_string_expr`/`call_wire_json_expr` at its own call
        // site (declared_header_lines skips it here; body_lines does too),
        // and a nested call inside a @body ctor field never carries
        // `.request` (Check_entry_ops.check_request_value only allows it as
        // a top-level trait value, never nested), so this position is
        // never actually reached for a `Call`.
        WireValue::Call(_) => {
            unreachable!("a header/uri/endpoint position never carries a top-level extern call")
        }
    }
}

/// A `path`/`uri` position: a literal or pure reference passes through
/// unescaped (the author took responsibility for its content by not writing
/// a template), while a template's placeholders are each percent-encoded as
/// one path segment.
fn push_uri_value(
    value: &WireValue,
    fields: &FieldCtx<'_>,
    fmt: &mut String,
    args: &mut Vec<String>,
) {
    match value {
        WireValue::Template(parts) => {
            for part in parts {
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
                    TemplatePart::Param(segs) => {
                        fmt.push_str("{}");
                        args.push(match segs.first() {
                            None => "percent_path(&input.to_string())".to_string(),
                            Some(name) => match fields.param_str_ref_expr(name) {
                                Some(access) => format!("percent_path({access})"),
                                None => format!("path_part(record.get({}))", rust_str(name)),
                            },
                        });
                    }
                }
            }
        }
        other => {
            fmt.push_str("{}");
            args.push(wire_value_expr(other, fields));
        }
    }
}

/// The URL: the typed read of the resolved endpoint value concatenated with
/// the URI value. `validate_entries` re-checks at generation time that the
/// binding carries an endpoint, so the read needs no runtime guard.
fn url_line(wire: &WireBinding, has_query: bool, fields: &FieldCtx<'_>) -> String {
    let endpoint = wire
        .endpoint
        .as_ref()
        .expect("validate_entries rejects an entry @http op with no endpoint");
    let mut fmt = String::from("{}");
    // The common case (a resolved entry-field endpoint) keeps its original
    // `Display`-only spelling; the grammar's other endpoint forms go
    // through the general renderer.
    let endpoint_expr = match endpoint {
        WireValue::Field(path) => fields.display_expr(path),
        other => wire_value_expr(other, fields),
    };
    let mut args = vec![endpoint_expr];
    push_uri_value(&wire.uri, fields, &mut fmt, &mut args);
    let binding = if has_query { "let mut url" } else { "let url" };
    format!("{binding} = format!(\"{fmt}\", {});\n", args.join(", "))
}

/// One `set_header(...)` call per declared `request_headers` entry whose
/// value is not a call: a call reads `.request`, which does not exist yet
/// at this point in assembly (headers are still being built) -- see
/// [`call_header_lines`], which patches those in once the request exists.
fn declared_header_lines(wire: &WireBinding, fields: &FieldCtx<'_>) -> String {
    wire.request_headers
        .iter()
        .filter(|(_, value)| !matches!(value, WireValue::Call(_)))
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

/// The query assembly: one `append_query(...)` per declared `@query` entry
/// (the same shared helper a query-bound record member used, now driven by
/// the key/value the trait declared instead of a member classification),
/// folded into `url` only when anything landed.
fn query_lines(wire: &WireBinding, fields: &FieldCtx<'_>) -> String {
    let mut out = String::from("let mut query: Vec<String> = Vec::new();\n");
    for (key, value) in &wire.query {
        let key_expr = template_expr(key, fields);
        let key_ref = if key_expr.starts_with('"') {
            key_expr
        } else {
            format!("&{key_expr}")
        };
        out.push_str(&format!(
            "append_query(&mut query, {key_ref}, Some(&{}));\n",
            wire_value_json_expr(value, fields)
        ));
    }
    out.push_str(
        "if !query.is_empty() {\n    url.push('?');\n    url.push_str(&query.join(\"&\"));\n}\n",
    );
    out
}

/// A `WireValue` position rendered as a `serde_json::Value`-producing
/// expression: a body ctor field, or nested inside one.
fn wire_value_json_expr(v: &WireValue, fields: &FieldCtx<'_>) -> String {
    match v {
        WireValue::Lit(json) => format!(
            "serde_json::from_str({}).unwrap_or(serde_json::Value::Null)",
            rust_str(&json.to_string())
        ),
        WireValue::Field(path) => format!(
            "serde_json::to_value(&{}).unwrap_or(serde_json::Value::Null)",
            fields.access(path)
        ),
        WireValue::Param(segs) => match segs.first() {
            None => "serde_json::to_value(&input).unwrap_or(serde_json::Value::Null)".to_string(),
            Some(name) => fields.param_json_expr(name).unwrap_or_else(|| {
                format!(
                    "record.get({}).cloned().unwrap_or(serde_json::Value::Null)",
                    rust_str(name)
                )
            }),
        },
        WireValue::Template(parts) => {
            let expr = template_expr(parts, fields);
            let owned = if expr.starts_with("format!") {
                expr
            } else {
                format!("{expr}.to_string()")
            };
            format!("serde_json::Value::String({owned})")
        }
        WireValue::Object(entries) => {
            let inserts: String = entries
                .iter()
                .map(|(k, val)| {
                    format!(
                        "m.insert({}.to_string(), {});\n",
                        rust_str(k),
                        wire_value_json_expr(val, fields)
                    )
                })
                .collect();
            format!(
                "{{\n{}    serde_json::Value::Object(m)\n}}",
                super::indent(
                    &format!("let mut m = serde_json::Map::new();\n{inserts}"),
                    1
                )
            )
        }
        // A top-level @body call is rendered directly by `call_wire_json_expr`
        // at its own call site (`body_lines` skips it here). A call nested
        // inside a ctor field would need to run before the request it might
        // read exists; `validate::wire_call_resolves` rejects that shape
        // upstream, so it never reaches this renderer.
        WireValue::Call(_) => unreachable!(
            "validate::wire_call_resolves rejects an extern call nested inside a ctor field"
        ),
    }
}

/// The request body statement, or `None` when the operation sends no body:
/// `wire.body` says exactly what the body is, never inferred from what the
/// input leaves undeclared. The whole-parameter form serializes the typed
/// input directly (its serde impl already spells the wire keys, `@wire`
/// renames included); a single member reads it raw off the record, absent
/// when the member itself is; every other form (an entry-field reference, a
/// template, or the @body ctor mapper) builds the body value explicitly.
fn body_lines(wire: &WireBinding, fields: &FieldCtx<'_>) -> Option<String> {
    let body = wire.body.as_ref()?;
    match body {
        // A top-level call reads `.request`, which does not exist yet at
        // this point in assembly; [`call_body_stmt`] patches it in once the
        // request is fully built (`body_field` renders "body: None" here in
        // the meantime).
        WireValue::Call(_) => None,
        WireValue::Param(segs) if segs.is_empty() => Some(format!(
            "let body = Some(serde_json::to_string(&input).map_err(|e| {})?);\n",
            encode_failure("e")
        )),
        WireValue::Param(segs) => {
            let name = segs
                .first()
                .expect("a @body param reference resolves zero or one segment deep");
            Some(format!(
                "let body = record.get({}).map(|v| v.to_string());\n",
                rust_str(name)
            ))
        }
        other => Some(format!(
            "let body = Some({}.to_string());\n",
            wire_value_json_expr(other, fields)
        )),
    }
}

/// The failure a failed input serialization maps to: a Config problem with
/// the value the caller supplied, not a Decode one (that category reads a
/// *response* against the declared schema). Near-unreachable for
/// derive-generated types, but the category still has to be right when it
/// fires.
fn encode_failure(cause: &str) -> String {
    format!(
        "TonoError::Config(ConfigError {{ message: format!(\"input serialization: {{{cause}}}\") }})"
    )
}

/// The Rust spelling of the shared [`success_test_expr`] rule.
fn success_expr(wire: &WireBinding) -> String {
    success_test_expr(wire, "outcome.status", "==")
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
    let body = body_lines(wire, fields);

    refs.push(super::support_symbol("HttpRequest"));
    let mut out = String::new();
    let resolves = |name: &str| fields.param(name).is_some();
    // Whether the op's own parameter type is touched anywhere: the
    // structural (never resolution-aware) reachability check, so a resolved
    // `input.field` read counts exactly like a `record`-indexed one would.
    let reads_input = needs_record(wire) || body.as_deref().is_some_and(|b| b.contains("&input"));
    if call.has_input && !reads_input {
        // An input no request position consumes (an empty struct bound to
        // nothing) is still part of the operation's declared signature; the
        // explicit discard keeps the generated method warning-clean.
        out.push_str("let _ = &input;\n");
    }
    if call.has_input
        && (needs_record_for_reads(wire, &resolves) || body_reads_record(wire, &resolves))
    {
        out.push_str(&format!(
            "let record = serde_json::to_value(&input).map_err(|e| {})?;\n",
            encode_failure("e")
        ));
    }
    push_gap(&mut out);
    out.push_str(&url_line(wire, query, fields));
    if query {
        out.push_str(&query_lines(wire, fields));
        refs.push(super::shared_symbol("append_query"));
    }
    push_gap(&mut out);
    out.push_str(
        "let mut headers: std::collections::HashMap<String, String> = std::collections::HashMap::new();\n",
    );
    refs.push(super::shared_symbol("set_header"));
    out.push_str(&declared_header_lines(wire, fields));
    out.push_str(
        "for (k, v) in &self.options.headers {\n    set_header(&mut headers, k, v.clone());\n}\n",
    );
    if out.contains("path_part(") {
        refs.push(super::shared_symbol("path_part"));
    }
    if out.contains("percent_path(") {
        refs.push(super::shared_symbol("percent_path"));
    }
    if out.contains("format_scalar(") {
        refs.push(super::shared_symbol("format_scalar"));
    }
    let body_field = match &body {
        Some(lines) => {
            push_gap(&mut out);
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
        push_gap(&mut out);
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
    // A call-valued @header/@body reads the request once it exists, so it
    // patches in right here: after the declared values are folded into
    // `request` (this line), right before it is sent.
    let call_request_lines = call_header_lines(wire, fields.module, fields, "request")
        + &call_body_stmt(wire, fields.module, fields, "request").unwrap_or_default();
    let request_binding = if call_request_lines.is_empty() {
        "let request"
    } else {
        "let mut request"
    };
    // A fresh headers copy per attempt: a call-valued header/body above may
    // rewrite the map/body it receives, and a retried attempt must not see a
    // prior attempt's rewrite.
    let mut attempt = format!(
        "{request_binding} = HttpRequest {{ method: {method}.to_string(), url: url.clone(), headers: headers.clone(), {body_field} }};\n",
        method = rust_str(call.method),
    );
    attempt.push_str(&call_request_lines);
    push_gap(&mut attempt);
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
    let bind = if fold { "response" } else { "outcome" };
    attempt.push_str(&format!(
        "let {bind} = match {send_call}.await {{\n    Ok(response) => response,\n{arm}}};\n",
        arm = super::indent(&transport_arm, 1),
    ));
    if fold {
        refs.push(super::shared_symbol("parse_json_object"));
        refs.push(super::support_symbol("HttpResponse"));
        attempt.push_str(&response_fold_lines(wire));
    }
    push_gap(&mut attempt);
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
    push_gap(&mut attempt);
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

    push_gap(&mut out);
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

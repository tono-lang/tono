//! The wire-position extern-call leaf: an extern call read as a
//! @header/@body value. Split out of `transport.rs` to keep that file's
//! leaf table from growing past the file-size gate; mirrors
//! `resolve_call.rs`'s split from `resolve.rs` for the entry-field
//! construction call.
//!
//! Argument resolution differs from `resolve_call.rs`: a wire-position
//! call's arguments resolve against the op's own scope (through
//! `FieldCtx`/the decoded record) and the assembled request, not an
//! entry's, and it carries the reserved [`WireCallArg::Request`] marker for
//! `.request` that an entry-field call's arguments never do.

use super::resolve_call::{find_extern, find_lib, find_rust_lang, json_literal};
use super::transport::{rust_str, template_expr, FieldCtx};
use crate::ir::{Module, Prim, Tref, WireBinding, WireCall, WireCallArg, WireValue};

/// One argument to a @header/@body-position extern call: an ordinary ref
/// resolves against the op's own scope exactly like a `WireValue` in the
/// same position would; the reserved [`WireCallArg::Request`] resolves to
/// `request_var`, the already-assembled request the call reads. `Ctor`
/// never reaches here: `validate::wire_call_resolves` rejects a
/// struct-literal mapper in this position (not yet supported by this
/// target's emitter).
fn call_arg_wire_expr(arg: &WireCallArg, fields: &FieldCtx<'_>, request_var: &str) -> String {
    match arg {
        WireCallArg::Request => format!("{request_var}.clone()"),
        WireCallArg::Field(path) => format!("{}.clone()", fields.access(path)),
        WireCallArg::Param(segs) => match segs.first() {
            None => "input.clone()".to_string(),
            Some(name) => fields
                .param(name)
                .map(|(access, _)| format!("{access}.clone()"))
                .unwrap_or_else(|| {
                    format!(
                        "record.get({}).cloned().unwrap_or(serde_json::Value::Null)",
                        rust_str(name)
                    )
                }),
        },
        WireCallArg::Lit(v) => json_literal(v),
        WireCallArg::Ctor(_) => unreachable!(
            "validate::wire_call_resolves rejects a ctor argument in a wire-position call"
        ),
    }
}

/// The bare call expression (crate-qualified symbol, args, `.await`) for a
/// @header/@body-position extern call. Mirrors
/// [`super::resolve_call::call_expr`], but arguments resolve against the
/// op's own scope (through `fields`) and the assembled request, not an
/// entry's; lookups are `expect`ed rather than diagnosed, exactly like that
/// sibling function -- `validate::wire_call_resolves` checks this ahead of
/// every Rust generation call.
fn call_wire_bare_expr(
    call: &WireCall,
    module: &Module,
    fields: &FieldCtx<'_>,
    request_var: &str,
) -> String {
    let lib = find_lib(module, &call.ns);
    let decl = find_extern(lib, &call.fn_name);
    let lang = find_rust_lang(decl);
    let crate_ident = lib
        .langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
        .expect("validate::wire_call_resolves checked a rust module path exists");
    let args: Vec<String> = call
        .args
        .iter()
        .map(|a| call_arg_wire_expr(a, fields, request_var))
        .collect();
    let awaited = if lang.sync { "" } else { ".await" };
    format!(
        "{crate_ident}::{}({}){awaited}",
        lang.symbol,
        args.join(", ")
    )
}

/// The `errors:` mapping for a wire-position call. A mapped sentinel would
/// ideally construct the declared error type the RFC's `errors:` line
/// names, the same category a bespoke hook's declared error reaches; the
/// `errors:` grammar only names a sentinel -> type, with no field mapping
/// to build an arbitrary shape from a raw error string, so every failure
/// here -- mapped or not -- currently reaches `Contract`, naming the
/// extern, mirroring `hook_lines`'s own unmapped fallback.
fn wire_call_error_wrap(call: &WireCall) -> String {
    let contract_name = format!("{}.{}", call.ns, call.fn_name);
    format!(
        "TonoError::Contract(ContractError {{ contract_name: {contract_name:?}.to_string(), cause: e.to_string().into() }})"
    )
}

/// A wire-position call as a `String`-typed expression (a header value):
/// the extern's declared return type decides whether the `Ok` binding needs
/// converting, mirroring [`FieldCtx::string_expr`]'s own String/Uuid split.
/// The call is inline as a `match` -- valid in expression position because
/// the `Err` arm diverges via `return` -- so the caller needs no
/// surrounding statement.
fn call_wire_string_expr(
    call: &WireCall,
    module: &Module,
    fields: &FieldCtx<'_>,
    request_var: &str,
) -> String {
    let bare = call_wire_bare_expr(call, module, fields, request_var);
    let lib = find_lib(module, &call.ns);
    let decl = find_extern(lib, &call.fn_name);
    let convert = match &decl.r#return {
        Tref::Prim(Prim::String | Prim::Uuid) => "v",
        _ => "v.to_string()",
    };
    format!(
        "match {bare} {{ Ok(v) => {convert}, Err(e) => return Err({}) }}",
        wire_call_error_wrap(call)
    )
}

/// A wire-position call as a `serde_json::Value`-producing expression (a
/// body value), mirroring [`call_wire_string_expr`].
pub(super) fn call_wire_json_expr(
    call: &WireCall,
    module: &Module,
    fields: &FieldCtx<'_>,
    request_var: &str,
) -> String {
    let bare = call_wire_bare_expr(call, module, fields, request_var);
    format!(
        "match {bare} {{ Ok(v) => serde_json::to_value(&v).unwrap_or(serde_json::Value::Null), Err(e) => return Err({}) }}",
        wire_call_error_wrap(call)
    )
}

/// One `set_header(&mut request.headers, ...)` per call-valued
/// `request_headers` entry: run once the request is fully assembled (the
/// declared values already folded in, see `transport::declared_header_lines`),
/// so the call's own `.request` argument (`request_var`) is the complete,
/// already-built request -- method, path, headers, and body -- matching
/// the same slot the `before_request` hook occupies (right before it, so a
/// hook still sees the signed header).
pub(super) fn call_header_lines(
    wire: &WireBinding,
    module: &Module,
    fields: &FieldCtx<'_>,
    request_var: &str,
) -> String {
    wire.request_headers
        .iter()
        .enumerate()
        .filter_map(|(i, (key, value))| match value {
            WireValue::Call(call) => {
                let key_expr = template_expr(key, fields);
                let key_ref = if key_expr.starts_with('"') {
                    key_expr
                } else {
                    format!("&{key_expr}")
                };
                let result_var = format!("signed{i}");
                // Bound to its own `let` first, not inlined into
                // `set_header`'s argument list: the call's own `.request`
                // argument borrows `request` (via `.clone()`) while
                // `&mut request.headers` is also live as an argument in the
                // same call expression, which the borrow checker rejects.
                Some(format!(
                    "let {result_var} = {};\nset_header(&mut {request_var}.headers, {key_ref}, {result_var});\n",
                    call_wire_string_expr(call, module, fields, request_var)
                ))
            }
            _ => None,
        })
        .collect()
}

/// The `request.body = ...` statement patching a call-valued `@body` in
/// once the request is fully assembled, mirroring [`call_header_lines`].
pub(super) fn call_body_stmt(
    wire: &WireBinding,
    module: &Module,
    fields: &FieldCtx<'_>,
    request_var: &str,
) -> Option<String> {
    match wire.body.as_ref()? {
        WireValue::Call(call) => Some(format!(
            "{request_var}.body = Some({}.to_string());\n",
            call_wire_json_expr(call, module, fields, request_var)
        )),
        _ => None,
    }
}

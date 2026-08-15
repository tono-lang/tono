//! The wire-position extern-call leaf: an extern call read as a
//! @header/@body value. Split out of `transport.rs` to keep that file's
//! leaf table from growing past the file-size gate.
//!
//! Argument resolution differs from `ext_call.rs`'s entry-field
//! construction call: a wire-position call's arguments resolve against the
//! op's own scope (through `field_expr`/`param_access`/the decoded record)
//! and the assembled request, not an entry's, and it carries the reserved
//! [`WireCallArg::Request`] marker for `.request` that an entry-field
//! call's arguments never do.

use crate::codegen::symbol::Symbol;
use crate::ir::{
    ExtLib, ExternDecl, ExternLang, Module, WireBinding, WireCall, WireCallArg, WireValue,
};

use super::module_symbol;
use super::transport::{indent, js_str, param_expr, template_expr, ParamAccess};

fn find_lib<'a>(module: &'a Module, ns: &str) -> &'a ExtLib {
    module
        .ext_libs
        .iter()
        .find(|l| l.name == ns)
        .expect("validate::wire_call_resolves checked this ext block exists")
}

fn find_extern<'a>(lib: &'a ExtLib, func: &str) -> &'a ExternDecl {
    lib.externs
        .iter()
        .find(|e| e.name == func)
        .expect("validate::wire_call_resolves checked this extern exists")
}

fn find_ts_lang(decl: &ExternDecl) -> &ExternLang {
    decl.langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
        .expect("validate::wire_call_resolves checked a ts block exists")
}

/// One argument to a @header/@body-position extern call: an ordinary ref
/// resolves against the op's own scope exactly like a `WireValue` in the
/// same position would; the reserved [`WireCallArg::Request`] resolves to
/// `request_var`, the already-assembled request the call reads. `Ctor`
/// never reaches here: `validate::wire_call_resolves` rejects a
/// struct-literal mapper in this position (not yet supported by this
/// target's emitter).
fn wire_call_arg_expr(
    arg: &WireCallArg,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
    param_access: ParamAccess<'_>,
    request_var: &str,
) -> String {
    match arg {
        WireCallArg::Request => request_var.to_string(),
        WireCallArg::Field(path) => field_expr(path),
        WireCallArg::Param(segs) => param_expr(segs, param_access, input_expr),
        WireCallArg::Lit(v) => match v.as_str() {
            Some(s) => js_str(s),
            None => v.to_string(),
        },
        WireCallArg::Ctor(_) => unreachable!(
            "validate::wire_call_resolves rejects a ctor argument in a wire-position call"
        ),
    }
}

/// The statement invoking a @header/@body-position extern call and binding
/// its result to `const {result_var}`: caught failures wrap into
/// `ContractError` naming the extern -- TypeScript's outer method boundary
/// already re-wraps an uncaught throw the same way (see `client.rs`), but a
/// wire-position call catches locally so the contract name reads the call
/// site, not the enclosing method. Unlike an entry field's own construction
/// call ([`super::ext_call`]), a declared sentinel is not yet projected to
/// its own typed error class here -- every failure reaches `ContractError`,
/// a scoped-down first pass.
#[allow(clippy::too_many_arguments)]
fn call_wire_stmt(
    call: &WireCall,
    module: &Module,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
    param_access: ParamAccess<'_>,
    request_var: &str,
    result_var: &str,
    refs: &mut Vec<Symbol>,
) -> String {
    let lib = find_lib(module, &call.ns);
    let decl = find_extern(lib, &call.fn_name);
    let lang = find_ts_lang(decl);
    let lib_path = lib
        .langs
        .iter()
        .find(|p| p.lang == "ts" || p.lang == "typescript")
        .expect("validate::wire_call_resolves checked a ts module path exists");
    refs.push(Symbol::imported(
        lang.symbol.clone(),
        lib_path.path.clone(),
        lang.symbol.clone(),
    ));
    refs.push(module_symbol(
        &crate::codegen::ops::error_names().contract,
        module,
    ));
    let args: Vec<String> = call
        .args
        .iter()
        .map(|a| wire_call_arg_expr(a, field_expr, input_expr, param_access, request_var))
        .collect();
    let contract = crate::codegen::ops::error_names().contract;
    let contract_name = format!("{}.{}", call.ns, call.fn_name);
    format!(
        "let {result_var};\ntry {{\n  {result_var} = await {symbol}({args});\n}} catch (e) {{\n  throw new {contract}({contract_name:?}, e);\n}}\n",
        symbol = lang.symbol,
        args = args.join(", "),
    )
}

/// One call-statement block per call-valued `request_headers` entry: run
/// once the request is fully assembled (the declared values already folded
/// in, see `transport::declared_header_lines`), so the call's own
/// `.request` argument (`request_var`) is the complete, already-built
/// request -- method, path, headers, and body -- matching the same slot
/// the `before_request` hook occupies (right before it, so a hook still
/// sees the signed header).
#[allow(clippy::too_many_arguments)]
pub(super) fn call_header_lines(
    wire: &WireBinding,
    module: &Module,
    indent_str: &str,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
    param_access: ParamAccess<'_>,
    request_var: &str,
    refs: &mut Vec<Symbol>,
) -> String {
    let mut out = String::new();
    for (i, (key, value)) in wire.request_headers.iter().enumerate() {
        let WireValue::Call(call) = value else {
            continue;
        };
        let result_var = format!("signed{i}");
        out.push_str(&indent(
            &call_wire_stmt(
                call,
                module,
                field_expr,
                input_expr,
                param_access,
                request_var,
                &result_var,
                refs,
            ),
            indent_str,
        ));
        out.push_str(&format!(
            "{indent_str}setHeader({request_var}.headers, {}, {result_var});\n",
            template_expr(key, field_expr, input_expr, param_access),
        ));
    }
    out
}

/// The `request.body = ...` statement patching a call-valued `@body` in
/// once the request is fully assembled, mirroring [`call_header_lines`].
#[allow(clippy::too_many_arguments)]
pub(super) fn call_body_stmt(
    wire: &WireBinding,
    module: &Module,
    indent_str: &str,
    field_expr: &dyn Fn(&[String]) -> String,
    input_expr: &str,
    param_access: ParamAccess<'_>,
    request_var: &str,
    refs: &mut Vec<Symbol>,
) -> String {
    let Some(WireValue::Call(call)) = wire.body.as_ref() else {
        return String::new();
    };
    let mut out = indent(
        &call_wire_stmt(
            call,
            module,
            field_expr,
            input_expr,
            param_access,
            request_var,
            "signedBody",
            refs,
        ),
        indent_str,
    );
    out.push_str(&format!("{indent_str}{request_var}.body = signedBody;\n"));
    out
}

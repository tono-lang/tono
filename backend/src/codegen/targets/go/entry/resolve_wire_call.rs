//! The wire-position extern-call leaf: an extern call read as a
//! @header/@body value. Split out of `transport.rs` to keep that file's
//! leaf table from growing past the file-size gate.
//!
//! Argument resolution differs from `ext.rs`'s entry-field construction
//! call: a wire-position call's arguments resolve against the op's own
//! scope (through `field_access`/`param_access`/the decoded record) and the
//! assembled request, not an entry's, and it carries the reserved
//! [`WireCallArg::Request`] marker for `.request` that an entry-field
//! call's arguments never do. Error handling reuses [`super::ext::error_block`]
//! directly: a mapped `errors:` sentinel becomes its declared SDK error
//! type, unmapped becomes `ContractError` naming the extern.

use crate::codegen::casing::CasingConfig;
use crate::codegen::symbol::Symbol;
use crate::ir::{
    ExtLib, ExternDecl, ExternLang, Module, WireBinding, WireCall, WireCallArg, WireValue,
};

use super::ext::{error_block, import_lib};
use super::transport::{go_json_lit, template_expr, FieldKind, ParamAccess, Reached};

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

fn find_go_lang(decl: &ExternDecl) -> &ExternLang {
    decl.langs
        .iter()
        .find(|l| l.lang == "go")
        .expect("validate::wire_call_resolves checked a go block exists")
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
    field_access: &dyn Fn(&[String]) -> String,
    param_access: ParamAccess<'_>,
    request_var: &str,
) -> String {
    match arg {
        WireCallArg::Request => request_var.to_string(),
        WireCallArg::Field(path) => field_access(path),
        WireCallArg::Param(segs) => match segs.first() {
            None => "input".to_string(),
            Some(name) => param_access(name)
                .map(|(access, _)| access)
                .unwrap_or_else(|| format!("record[{name:?}]")),
        },
        WireCallArg::Lit(v) => go_json_lit(v),
        WireCallArg::Ctor(_) => unreachable!(
            "validate::wire_call_resolves rejects a ctor argument in a wire-position call"
        ),
    }
}

/// The Go statement(s) invoking a @header/@body-position extern call and
/// binding its result to `result_var`: the call itself, then
/// [`error_block`]'s sentinel discrimination, returning early on failure so
/// `result_var` is always valid past this point.
#[allow(clippy::too_many_arguments)]
fn call_wire_stmt(
    call: &WireCall,
    module: &Module,
    config: &CasingConfig,
    field_access: &dyn Fn(&[String]) -> String,
    param_access: ParamAccess<'_>,
    request_var: &str,
    result_var: &str,
    ret_zero: &str,
    fail: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let lib = find_lib(module, &call.ns);
    let decl = find_extern(lib, &call.fn_name);
    let lang = find_go_lang(decl);
    let callee = import_lib(refs, lib).unwrap_or_default();
    let args: Vec<String> = call
        .args
        .iter()
        .map(|a| wire_call_arg_expr(a, field_access, param_access, request_var))
        .collect();
    let err_var = format!("{result_var}Err");
    let mut out = format!(
        "{result_var}, {err_var} := {callee}.{}({})\n",
        lang.symbol,
        args.join(", ")
    );
    let contract_name = format!("{}.{}", call.ns, call.fn_name);
    out.push_str(&error_block(
        refs,
        module,
        config,
        lib,
        &lang.errors,
        &contract_name,
        &err_var,
        &|expr| format!("return {ret_zero}{}\n", fail(expr)),
    ));
    out
}

/// One call-statement block per call-valued `request_headers` entry: run
/// once the request is fully assembled (the declared values already folded
/// in, see `transport::header_lines`), so the call's own `.request`
/// argument (`request_var`) is the complete, already-built request --
/// method, path, headers, and body.
#[allow(clippy::too_many_arguments)]
pub(super) fn call_header_lines(
    wire: &WireBinding,
    module: &Module,
    config: &CasingConfig,
    field_access: &dyn Fn(&[String]) -> String,
    field_kind: &dyn Fn(&[String]) -> FieldKind,
    param_access: ParamAccess<'_>,
    request_var: &str,
    ret_zero: &str,
    fail: &dyn Fn(String) -> String,
    reached: &mut Reached,
    refs: &mut Vec<Symbol>,
) -> String {
    let set = reached.slot("SetHeader");
    let mut out = String::new();
    for (i, (key, value)) in wire.request_headers.iter().enumerate() {
        let WireValue::Call(call) = value else {
            continue;
        };
        let result_var = format!("signed{i}");
        out.push_str(&call_wire_stmt(
            call,
            module,
            config,
            field_access,
            param_access,
            request_var,
            &result_var,
            ret_zero,
            fail,
            refs,
        ));
        out.push_str(&format!(
            "\t{set}({request_var}.Headers, {}, {result_var})\n",
            template_expr(key, false, field_access, field_kind, param_access, reached),
        ));
    }
    out
}

/// The `request.Body = ...` statement patching a call-valued `@body` in once
/// the request is fully assembled, mirroring [`call_header_lines`]. Bytes,
/// not a string: `Request.Body` is `[]byte` (see `send.rs`'s `Request`
/// struct), matching every other body-building path in `transport.rs`.
#[allow(clippy::too_many_arguments)]
pub(super) fn call_body_stmt(
    wire: &WireBinding,
    module: &Module,
    config: &CasingConfig,
    field_access: &dyn Fn(&[String]) -> String,
    param_access: ParamAccess<'_>,
    request_var: &str,
    ret_zero: &str,
    fail: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let Some(WireValue::Call(call)) = wire.body.as_ref() else {
        return String::new();
    };
    let mut out = call_wire_stmt(
        call,
        module,
        config,
        field_access,
        param_access,
        request_var,
        "signedBody",
        ret_zero,
        fail,
        refs,
    );
    out.push_str(&format!("\t{request_var}.Body = []byte(signedBody)\n"));
    out
}

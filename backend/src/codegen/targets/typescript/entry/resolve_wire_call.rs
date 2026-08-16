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
/// request -- method, path, headers, and body -- right before it is sent.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        CallArg, ExtLib, ExternDecl, ExternLang, ExternParam, LangPath, Prim, TemplatePart, Tref,
    };

    fn module_with_sign_extern() -> Module {
        Module {
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![],
            tests: vec![],
            ext_libs: vec![ExtLib {
                name: "companyauth".into(),
                langs: vec![LangPath {
                    lang: "ts".into(),
                    path: "@company/auth".into(),
                }],
                structs: vec![],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "sign".into(),
                    params: vec![ExternParam {
                        name: "request".into(),
                        r#type: Tref::Prim(Prim::String),
                    }],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![ExternLang {
                        lang: "ts".into(),
                        symbol: "sign".into(),
                        call_args: vec![CallArg::Ref(vec!["request".into()])],
                        yields: vec![],
                        returns: None,
                        errors: vec![],
                        sync: false,
                        infallible: false,
                    }],
                }],
            }],
        }
    }

    fn field_expr(path: &[String]) -> String {
        format!("this.settings.{}", path.join("."))
    }

    fn no_param_access(_: &str) -> Option<String> {
        None
    }

    fn call() -> WireCall {
        WireCall {
            ns: "companyauth".into(),
            fn_name: "sign".into(),
            args: vec![WireCallArg::Request],
        }
    }

    #[test]
    fn a_request_argument_reads_the_assembled_request_variable() {
        let out = wire_call_arg_expr(
            &WireCallArg::Request,
            &field_expr,
            "input",
            &no_param_access,
            "request",
        );
        assert_eq!(out, "request");
    }

    #[test]
    fn a_field_argument_reads_through_the_field_expr_closure() {
        let out = wire_call_arg_expr(
            &WireCallArg::Field(vec!["id".into()]),
            &field_expr,
            "input",
            &no_param_access,
            "request",
        );
        assert_eq!(out, "this.settings.id");
    }

    #[test]
    fn a_string_literal_argument_renders_as_a_js_string() {
        let out = wire_call_arg_expr(
            &WireCallArg::Lit(serde_json::json!("v")),
            &field_expr,
            "input",
            &no_param_access,
            "request",
        );
        assert_eq!(out, "\"v\"");
    }

    #[test]
    fn a_non_string_literal_argument_renders_as_raw_json() {
        let out = wire_call_arg_expr(
            &WireCallArg::Lit(serde_json::json!(3)),
            &field_expr,
            "input",
            &no_param_access,
            "request",
        );
        assert_eq!(out, "3");
    }

    #[test]
    fn call_wire_stmt_awaits_the_imported_symbol_inside_a_try_catch() {
        let module = module_with_sign_extern();
        let mut refs = Vec::new();
        let out = call_wire_stmt(
            &call(),
            &module,
            &field_expr,
            "input",
            &no_param_access,
            "request",
            "signed0",
            &mut refs,
        );
        assert!(out.contains("let signed0;"), "{out}");
        assert!(out.contains("signed0 = await sign(request);"), "{out}");
        assert!(out.contains("} catch (e) {"), "{out}");
        assert!(out.contains("\"companyauth.sign\""), "{out}");
        assert!(!refs.is_empty());
    }

    #[test]
    fn call_header_lines_emits_the_call_then_set_header_per_entry() {
        let module = module_with_sign_extern();
        let mut refs = Vec::new();
        let wire = WireBinding {
            method: "GET".into(),
            uri: WireValue::Template(vec![]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: vec![(
                vec![TemplatePart::Lit("Authorization".into())],
                WireValue::Call(call()),
            )],
            query: vec![],
            timeout: None,
            retry: None,
        };
        let out = call_header_lines(
            &wire,
            &module,
            "  ",
            &field_expr,
            "input",
            &no_param_access,
            "request",
            &mut refs,
        );
        assert!(out.contains("signed0 = await sign(request);"), "{out}");
        assert!(
            out.contains("setHeader(request.headers, \"Authorization\", signed0);"),
            "{out}"
        );
    }

    #[test]
    fn call_header_lines_is_empty_with_no_call_valued_header() {
        let module = module_with_sign_extern();
        let mut refs = Vec::new();
        let wire = WireBinding {
            method: "GET".into(),
            uri: WireValue::Template(vec![]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: vec![],
            query: vec![],
            timeout: None,
            retry: None,
        };
        let out = call_header_lines(
            &wire,
            &module,
            "  ",
            &field_expr,
            "input",
            &no_param_access,
            "request",
            &mut refs,
        );
        assert_eq!(out, "");
    }

    #[test]
    fn call_body_stmt_assigns_the_signed_value_to_request_body() {
        let module = module_with_sign_extern();
        let mut refs = Vec::new();
        let wire = WireBinding {
            method: "POST".into(),
            uri: WireValue::Template(vec![]),
            body: Some(WireValue::Call(call())),
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: vec![],
            query: vec![],
            timeout: None,
            retry: None,
        };
        let out = call_body_stmt(
            &wire,
            &module,
            "  ",
            &field_expr,
            "input",
            &no_param_access,
            "request",
            &mut refs,
        );
        assert!(out.contains("request.body = signedBody;"), "{out}");
    }

    #[test]
    fn call_body_stmt_is_empty_without_a_call_valued_body() {
        let module = module_with_sign_extern();
        let mut refs = Vec::new();
        let wire = WireBinding {
            method: "POST".into(),
            uri: WireValue::Template(vec![]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: vec![],
            query: vec![],
            timeout: None,
            retry: None,
        };
        let out = call_body_stmt(
            &wire,
            &module,
            "  ",
            &field_expr,
            "input",
            &no_param_access,
            "request",
            &mut refs,
        );
        assert_eq!(out, "");
    }
}

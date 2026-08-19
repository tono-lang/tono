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
/// ideally construct the declared error type the `errors:` line names; the
/// `errors:` grammar only names a sentinel -> type, with no field mapping
/// to build an arbitrary shape from a raw error string, so every failure
/// here -- mapped or not -- currently reaches `Contract`, naming the
/// extern.
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
/// already-built request -- method, path, headers, and body -- right before
/// it is sent.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::entries::module_entries;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::ir::{
        CallArg, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam, LangPath, Shape,
        ShapeKind, Source, TemplatePart,
    };

    fn field(name: &str, target: Tref) -> EntryField {
        EntryField {
            name: name.into(),
            target,
            sources: vec![Source::Arg],
            format: None,
            transforms: vec![],
            select: None,
            call: None,
            handle_call: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        }
    }

    fn module_with_extern(return_type: Tref, sync: bool) -> Module {
        Module {
            name: "m".into(),
            shapes: vec![Shape {
                id: "m#client".into(),
                kind: ShapeKind::Entry {
                    fields: vec![field("id", Tref::Prim(Prim::String))],
                    operations: vec![],
                },
                traits: vec![],
            }],
            operations: vec![],
            extensions: vec![],
            tests: vec![],
            ext_libs: vec![ExtLib {
                name: "companyauth".into(),
                langs: vec![LangPath {
                    lang: "rust".into(),
                    path: "company-auth".into(),
                }],
                structs: vec![],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "sign".into(),
                    params: vec![ExternParam {
                        name: "request".into(),
                        r#type: Tref::Prim(Prim::String),
                    }],
                    r#return: return_type,
                    langs: vec![ExternLang {
                        lang: "rust".into(),
                        symbol: "Client::sign".into(),
                        call_args: vec![CallArg::Ref(vec!["request".into()])],
                        yields: vec![],
                        returns: None,
                        errors: vec![],
                        sync,
                        infallible: false,
                        ctx: false,
                    }],
                }],
            }],
        }
    }

    fn with_ctx<R>(module: &Module, f: impl FnOnce(&FieldCtx<'_>) -> R) -> R {
        let entries = module_entries(module);
        let config = rust_casing();
        let ctx = FieldCtx {
            entry: &entries[0],
            module,
            config: &config,
            input: None,
        };
        f(&ctx)
    }

    fn call() -> WireCall {
        WireCall {
            ns: "companyauth".into(),
            fn_name: "sign".into(),
            args: vec![WireCallArg::Request],
        }
    }

    #[test]
    fn a_request_argument_clones_the_assembled_request() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_arg_wire_expr(&WireCallArg::Request, ctx, "request")
        });
        assert_eq!(out, "request.clone()");
    }

    #[test]
    fn a_field_argument_clones_the_typed_settings_read() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_arg_wire_expr(&WireCallArg::Field(vec!["id".into()]), ctx, "request")
        });
        assert_eq!(out, "self.settings.id.clone()");
    }

    #[test]
    fn an_unresolved_param_argument_falls_back_to_the_record() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_arg_wire_expr(&WireCallArg::Param(vec!["id".into()]), ctx, "request")
        });
        assert_eq!(
            out,
            "record.get(\"id\").cloned().unwrap_or(serde_json::Value::Null)"
        );
    }

    #[test]
    fn a_bare_param_argument_with_no_segments_reads_the_whole_input() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_arg_wire_expr(&WireCallArg::Param(vec![]), ctx, "request")
        });
        assert_eq!(out, "input.clone()");
    }

    #[test]
    fn a_literal_argument_renders_as_a_json_literal() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_arg_wire_expr(&WireCallArg::Lit(serde_json::json!("v")), ctx, "request")
        });
        assert!(out.contains("\\\"v\\\"") || out.contains('v'), "{out}");
    }

    #[test]
    fn the_bare_call_expression_uses_the_crate_ident_and_awaits_when_async() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_wire_bare_expr(&call(), &module, ctx, "request")
        });
        assert_eq!(out, "company_auth::Client::sign(request.clone()).await");
    }

    #[test]
    fn a_sync_extern_emits_the_bare_call_without_await() {
        let module = module_with_extern(Tref::Prim(Prim::String), true);
        let out = with_ctx(&module, |ctx| {
            call_wire_bare_expr(&call(), &module, ctx, "request")
        });
        assert_eq!(out, "company_auth::Client::sign(request.clone())");
    }

    #[test]
    fn the_error_wrap_names_the_extern_as_the_contract() {
        let out = wire_call_error_wrap(&call());
        assert!(out.contains("\"companyauth.sign\""), "{out}");
        assert!(out.contains("TonoError::Contract"), "{out}");
    }

    #[test]
    fn a_string_returning_call_binds_ok_directly() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_wire_string_expr(&call(), &module, ctx, "request")
        });
        assert!(out.contains("Ok(v) => v"), "{out}");
        assert!(out.contains("Err(e) => return Err("), "{out}");
    }

    #[test]
    fn a_non_string_returning_call_converts_before_binding() {
        let module = module_with_extern(Tref::Prim(Prim::I32), false);
        let out = with_ctx(&module, |ctx| {
            call_wire_string_expr(&call(), &module, ctx, "request")
        });
        assert!(out.contains("Ok(v) => v.to_string()"), "{out}");
    }

    #[test]
    fn a_json_expr_wraps_the_ok_value_through_serde_json() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let out = with_ctx(&module, |ctx| {
            call_wire_json_expr(&call(), &module, ctx, "request")
        });
        assert!(out.contains("serde_json::to_value(&v)"), "{out}");
    }

    #[test]
    fn call_header_lines_binds_a_let_before_set_header() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let mut wire = WireBinding {
            method: "GET".into(),
            uri: WireValue::Template(vec![]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: Some(WireValue::Field(vec!["id".into()])),
            request_headers: vec![(
                vec![TemplatePart::Lit("Authorization".into())],
                WireValue::Call(call()),
            )],
            query: vec![],
            timeout: None,
            retry: None,
        };
        let out = with_ctx(&module, |ctx| {
            call_header_lines(&wire, &module, ctx, "request")
        });
        assert!(out.contains("let signed0 ="), "{out}");
        assert!(
            out.contains("set_header(&mut request.headers, \"Authorization\", signed0);"),
            "{out}"
        );
        wire.request_headers.clear();
        let empty = with_ctx(&module, |ctx| {
            call_header_lines(&wire, &module, ctx, "request")
        });
        assert_eq!(empty, "");
    }

    #[test]
    fn call_body_stmt_is_none_without_a_call_valued_body() {
        let module = module_with_extern(Tref::Prim(Prim::String), false);
        let mut wire = WireBinding {
            method: "POST".into(),
            uri: WireValue::Template(vec![]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: Some(WireValue::Field(vec!["id".into()])),
            request_headers: vec![],
            query: vec![],
            timeout: None,
            retry: None,
        };
        assert!(with_ctx(&module, |ctx| call_body_stmt(
            &wire, &module, ctx, "request"
        ))
        .is_none());
        wire.body = Some(WireValue::Call(call()));
        let out = with_ctx(&module, |ctx| {
            call_body_stmt(&wire, &module, ctx, "request")
        })
        .unwrap();
        assert!(out.contains("request.body = Some("), "{out}");
        assert!(out.contains("serde_json::to_value(&v)"), "{out}");
    }
}

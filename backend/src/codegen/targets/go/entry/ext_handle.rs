//! The foreign opaque handle's generated Go surface: the unexported
//! method-set interface a field's storage position is declared with
//! ([`handle_iface_decl`]) and the adapter that lets the real construction
//! call's own concrete value satisfy it ([`handle_adapter_decl`]), so a
//! hermetic declared test can fake the interface without the real library.
//! Split out of `ext` to keep it under the file-size ceiling; `use super::*`
//! reaches the parent's call/yields/errors/returns machinery
//! (`build_call`, `error_block`, `returns_expr`, `go_lang`, ...).

use super::*;

/// One declared method's Go-rendered parameter list (`name Type`, in
/// declared order) and return type, with every referenced type registered
/// into `refs` along the way: shared by [`handle_iface_decl`] and
/// [`handle_adapter_decl`], whose interface method and adapter method
/// signatures must render byte-for-byte the same so the adapter actually
/// satisfies the interface.
fn method_signature(m: &ExternDecl, lang: &ExternLang, refs: &mut Vec<Symbol>) -> (Vec<String>, String) {
    let mut params: Vec<String> = Vec::new();
    if lang.ctx {
        refs.push(import("context", "context"));
        params.push("ctx context.Context".to_string());
    }
    params.extend(m.params.iter().map(|p| {
        push_type_symbols(&p.r#type, refs);
        format!("{} {}", camel(&p.name), go_type(&p.r#type))
    }));
    push_type_symbols(&m.r#return, refs);
    (params, go_type(&m.r#return))
}

/// The unexported name tono generates for a foreign opaque handle's own
/// method-set interface, qualified by lib and type so two libs' same-named
/// handle never collide in one Go package.
pub(in super::super) fn handle_iface_ident(lib_name: &str, type_name: &str) -> String {
    format!("{}{}Iface", camel(lib_name), pascal(type_name))
}

/// The interface type a foreign handle *field* is declared with (the
/// Settings/carrier storage position), in place of the real concrete pointer
/// type: a hermetic declared test fakes this interface instead of the real
/// library. The `@with` setter parameter keeps the concrete type for
/// consumer ergonomics (see [`handle_go_type`]); only the field's own static
/// type changes.
pub(in super::super) fn handle_iface_type(lib: &ExtLib, type_name: &str) -> Option<String> {
    lib_go_path(lib)?;
    Some(handle_iface_ident(&lib.name, type_name))
}

/// The unexported name of the adapter that lets the real value a handle
/// field's own construction call returns satisfy the field's own generated
/// interface: the real concrete type's methods return the *foreign*
/// package's own types, which the interface (spelled in logical/canonical
/// types) never accepts directly, so the real value is wrapped rather than
/// stored as-is.
pub(in super::super) fn handle_adapter_ident(lib_name: &str, type_name: &str) -> String {
    format!("{}Adapter", handle_iface_ident(lib_name, type_name))
}

/// The adapter type: one method per declared Go-bound handle method, each
/// calling the wrapped real value and running the exact same
/// yields/returns/errors projection an op's own `impl` call site runs
/// ([`build_call`]/[`returns_expr`]/[`error_block`]), so the interface's
/// signature never leaks a foreign type or a foreign error. `None` when the
/// handle declares no Go-bound method (nothing to adapt).
pub(in super::super) fn handle_adapter_decl(
    module: &Module,
    config: &CasingConfig,
    lib: &ExtLib,
    handle: &OpaqueType,
) -> Option<Decl> {
    let adapter_ty = handle_adapter_ident(&lib.name, &handle.name);
    let mut refs = Vec::new();
    let real_ty = handle_go_type(lib, handle, &mut refs)?;
    // The `real` field's own type names the foreign package directly (unlike
    // every other position this handle reaches, which is spelled as tono's
    // generated interface); the adapter decl is the one place that import is
    // actually used, so it is registered here rather than wherever the
    // storage-typed interface spelling is pushed instead. This also covers a
    // handle with no declared `errors:` mapping at all: previously the import
    // only got pushed as a side effect of an error-sentinel reference, so a
    // handle with no such mapping compiled to an undefined package selector.
    if let Some(sym) = handle_symbol(lib) {
        refs.push(sym);
    }
    let mut methods = String::new();
    for m in &handle.methods {
        let Some(lang) = go_lang(m) else { continue };
        let (params, ret_ty) = method_signature(m, lang, &mut refs);
        // The adapter's own parameters are exactly the method's declared
        // params, so a `Param` argument in the real call template resolves
        // to the adapter's own identically-named parameter.
        // Each identity arg names the adapter method's own Go parameter by a
        // single-segment `Ref` (never `Param`): the real call template's own
        // `Param(name)` entries resolve through this list by declared-param
        // position, so reusing `Param` here would resolve right back into
        // itself.
        let identity_args: Vec<CallArg> = m
            .params
            .iter()
            .map(|p| CallArg::Ref(vec![p.name.clone()]))
            .collect();
        let mut ref_expr = |path: &[String]| path.first().map(|s| camel(s)).unwrap_or_default();
        let built = build_call(
            &mut refs,
            lib,
            lang,
            "a.real",
            None,
            &m.params,
            &identity_args,
            &m.name,
            &mut ref_expr,
        );
        let mut body = format!("\tvar zero {ret_ty}\n");
        body.push_str(&built.stmt);
        if let Some(err_var) = &built.err_var {
            body.push_str(&error_block(
                &mut refs,
                module,
                config,
                lib,
                &lang.errors,
                &format!("{}.{}.{}", lib.name, handle.name, m.name),
                err_var,
                &|expr| format!("return zero, {expr}"),
            ));
        }
        match &lang.returns {
            None => {
                let value = built
                    .yields_vars
                    .get("")
                    .cloned()
                    .or_else(|| built.yields_vars.values().next().cloned())
                    .unwrap_or_else(|| "zero".to_string());
                body.push_str(&format!("\treturn {value}, nil\n"));
            }
            Some(returns) => {
                let (pre, expr) =
                    returns_expr(module, config, returns, &built.yields_vars, &m.name);
                body.push_str(&pre);
                body.push_str(&format!("\treturn {expr}, nil\n"));
            }
        }
        methods.push_str(&format!(
            "\nfunc (a *{adapter_ty}) {}({}) ({ret_ty}, error) {{\n{body}}}\n",
            lang.symbol,
            params.join(", "),
        ));
    }
    if methods.is_empty() {
        return None;
    }
    Some(Decl::raw_with(
        format!(
            "// {adapter_ty} lets the real value {lib}.{ty}'s own construction call\n\
             // returns satisfy {iface}: every method runs the same\n\
             // yields/returns/errors projection an op's own impl call site runs, so\n\
             // the interface's signature never leaks a foreign type.\n\
             type {adapter_ty} struct {{\n\treal {real_ty}\n}}\n{methods}",
            lib = lib.name,
            ty = handle.name,
            iface = handle_iface_ident(&lib.name, &handle.name),
        ),
        refs,
    ))
}

/// The interface declaration for one opaque handle: one method per declared
/// Go-bound method, named and signed exactly as [`build_call`] calls it (the
/// declared params and return type, both already the logical/canonical
/// types, never the foreign package's own), so nothing foreign leaks through
/// the interface's own signature. `None` when the handle declares no
/// Go-bound method (nothing to fake).
pub(in super::super) fn handle_iface_decl(lib: &ExtLib, handle: &OpaqueType) -> Option<Decl> {
    lib_go_path(lib)?;
    let ident = handle_iface_ident(&lib.name, &handle.name);
    let mut refs = Vec::new();
    let mut methods = String::new();
    for m in &handle.methods {
        let Some(lang) = go_lang(m) else { continue };
        let (params, ret_ty) = method_signature(m, lang, &mut refs);
        methods.push_str(&format!(
            "\t{}({}) ({}, error)\n",
            lang.symbol,
            params.join(", "),
            ret_ty,
        ));
    }
    if methods.is_empty() {
        return None;
    }
    Some(Decl::raw_with(
        format!(
            "// {ident} is the method set tono generates for the {lib}.{ty} handle: a\n\
             // hermetic declared test fakes it directly, without the real library.\n\
             type {ident} interface {{\n{methods}}}",
            lib = lib.name,
            ty = handle.name,
        ),
        refs,
    ))
}

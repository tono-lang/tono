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
/// declared order, `ctx context.Context` first when the binding declares a
/// context position) and return type, with every referenced type registered
/// into `refs` along the way: shared by [`handle_iface_decl`],
/// [`handle_adapter_decl`] and a declared test's fake handle
/// (`vector_extern::handle_fake_decl`), whose interface, adapter and fake
/// method signatures must render byte-for-byte the same so the adapter and
/// the fake actually satisfy the interface.
pub(in super::super) fn method_signature(
    m: &ExternDecl,
    lang: &ExternLang,
    refs: &mut Vec<Symbol>,
) -> (Vec<String>, String) {
    let mut params: Vec<String> = Vec::new();
    if binds_ctx(lang) {
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
    let real_ty = handle_go_type(lib, handle, module, &mut refs)?;
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
            module,
            lib,
            lang,
            &Callee::Receiver("a.real".to_string()),
            &m.params,
            &identity_args,
            &m.name,
            "ctx",
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
                &m.errors,
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

/// A field's own `= .field.method(args)` construction source: a call into
/// a sibling foreign handle's method, stored into the field's slot. The
/// receiver field's own static type is tono's generated interface (see
/// `field_go_type_storage`), so this is a plain interface call: its
/// methods are already signed in logical types and whatever backs it (the
/// real adapter, or a hermetic test's fake) already ran the
/// `yields`/`returns`/`errors` projection once, behind the interface, the
/// same reasoning an op's own `impl` body ([`impl_call_body`]) rests on.
/// A failure surfaces as the constructor's own error return: the adapter
/// has already mapped it onto the closed taxonomy.
///
/// A `ctx`-marked method receives `context.Background()`: the generated
/// constructor takes no caller context (its signature is positional `@arg`
/// values plus options), and construction is the one-shot resolution the
/// field exists for, so there is no deadline to thread through. The
/// method keeps its idiomatic `ctx context.Context` slot; only the value
/// filling it is fixed here.
pub(in super::super) fn handle_call_assign(
    r: &mut Resolver<'_, '_>,
    field: &EntryField,
    call: &crate::ir::OpImplCall,
    dest: &str,
) -> String {
    let Some(head) = call.recv.first() else {
        return format!("// handle call with no receiver\n{dest} = nil\n");
    };
    let Some(recv_field) = r.entry.fields.iter().find(|f| f.name == *head) else {
        return format!("// unresolved receiver field {head:?}\n{dest} = nil\n");
    };
    let Some((lib, handle_ty)) = foreign_handle(&recv_field.target, r.module) else {
        return format!("// {head:?} is not a foreign handle field\n{dest} = nil\n");
    };
    // `foreign_handle` only answers when the lib declares the type.
    let handle = lib
        .types
        .iter()
        .find(|t| t.name == handle_ty)
        .expect("foreign_handle resolved this type against the same lib");
    let Some(decl) = handle.methods.iter().find(|m| m.name == call.method) else {
        return format!(
            "// unresolved method {:?} on {handle_ty:?}\n{dest} = nil\n",
            call.method
        );
    };
    let Some(lang) = go_lang(decl) else {
        return format!(
            "// {:?} declares no Go binding\n{dest} = nil\n",
            call.method
        );
    };
    let (entry, module, config) = (r.entry, r.module, r.config);
    let recv_expr = field_path_expr(entry, module, config, &call.recv, "s");
    let mut ref_expr = move |path: &[String]| sibling_path_expr(entry, module, config, path);
    if binds_ctx(lang) {
        r.refs.push(import("context", "context"));
    }
    let call_args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| {
            call_arg_expr(
                r.refs,
                module,
                lib,
                a,
                &decl.params,
                &call.args,
                BACKGROUND_CTX,
                &mut ref_expr,
            )
        })
        .collect();
    let prefix = camel(&field.name);
    let out_var = format!("{prefix}Out");
    let err_var = format!("{prefix}Err");
    format!(
        "{out_var}, {err_var} := {recv_expr}.{}({})\n\
         if {err_var} != nil {{\n\treturn nil, {err_var}\n}}\n\
         {dest} = {out_var}\n",
        lang.symbol,
        call_args.join(", "),
    )
}

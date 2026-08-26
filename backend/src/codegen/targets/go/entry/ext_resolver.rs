//! One named function per foreign construction, in a file of its own per
//! `ext` library.
//!
//! A field constructed through a library (`formula = mathkit.from_formula(
//! .expr, .precision)`) is resolved by `resolveFormula(expr, precision)`: the
//! function takes exactly the settings the declaration reads and returns the
//! library's own value, with the declared error mapping inside. The
//! constructor is then the sequence of those calls in resolution order, and
//! a generated test builds its own sequence, putting a fake where a call's
//! result would go, so no runtime branch ever decides between the two.
//!
//! The resolver returns the library's value, not the adapter: the site that
//! stores the value wraps it, and a handle forwarded into another call never
//! enters the settings at all (it is handed straight to the consuming
//! resolver), which is what lets one foreign call feed another without a
//! type assertion.

use super::ext::{
    binds_ctx, build_call, error_block, find_extern, find_lib, foreign_handle, go_lang,
    handle_adapter_ident, handle_go_type, import_lib, returns_expr, Callee, BACKGROUND_CTX,
};
use super::*;
use crate::codegen::entries::{call_deps, TailStep};
use crate::ir::ExtLib;

/// A resolver function, with the `ext` library it calls into (which decides
/// the file it lands in).
pub(super) struct ResolverDecl {
    pub(super) lib: String,
    pub(super) decl: Decl,
}

/// The name of the function resolving a field's foreign construction
/// (`resolveFormula`), entry-prefixed in a multi-entry module like every other
/// per-entry companion.
pub(super) fn resolver_name(entry: &EntryModel<'_>, field: &EntryField, multi: bool) -> String {
    if multi {
        format!("resolve{}{}", pascal(entry.name), pascal(&field.name))
    } else {
        format!("resolve{}", pascal(&field.name))
    }
}

/// The Go keywords and the constructor's own locals a field's camelCase name
/// must not shadow when it becomes a local variable or a parameter.
const RESERVED_LOCALS: [&str; 31] = [
    "break",
    "case",
    "chan",
    "const",
    "continue",
    "default",
    "defer",
    "else",
    "fallthrough",
    "for",
    "func",
    "go",
    "goto",
    "if",
    "import",
    "interface",
    "map",
    "package",
    "range",
    "return",
    "select",
    "struct",
    "switch",
    "type",
    "var",
    "s",
    "w",
    "err",
    "opts",
    "ctx",
    "zero",
];

/// The local a resolved value binds to (and the parameter name a resolver
/// takes it under): the field's camelCase name, suffixed when that name is a
/// Go keyword or one of the constructor's own locals.
pub(super) fn local_ident(name: &str) -> String {
    let ident = camel(name);
    if RESERVED_LOCALS.contains(&ident.as_str()) {
        format!("{ident}Value")
    } else {
        ident
    }
}

/// The library and handle type of a field whose resolver returns the
/// library's own value: a foreign handle constructed by a free call with no
/// `returns:` projection. The storing site wraps that value in the handle's
/// adapter; every other resolver already returns the field's own type.
pub(super) fn raw_handle<'a>(
    field: &EntryField,
    module: &'a Module,
) -> Option<(&'a ExtLib, String)> {
    let call = field.call.as_ref()?;
    let lib = find_lib(module, &call.ns)?;
    let decl = find_extern(lib, &call.func)?;
    let lang = go_lang(decl)?;
    if lang.returns.is_some() {
        return None;
    }
    foreign_handle(&field.target, module)
}

/// A sibling-field path read inside a resolver body: the head is the
/// parameter the resolver takes it under, every later segment a plain member
/// access into whatever that field resolved to.
fn param_path_expr(config: &CasingConfig, path: &[String]) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        if i == 0 {
            out.push_str(&local_ident(seg));
        } else {
            out.push('.');
            out.push_str(&field_pascal(seg, config));
        }
    }
    out
}

/// The Go type a resolver takes a dependency under: a forwarded handle
/// arrives as the library's own value (the raw result of its own resolver),
/// everything else as its storage type (a stored handle as tono's generated
/// interface, so the same resolver runs over the real adapter or a fake).
fn dep_type(
    entry: &EntryModel<'_>,
    module: &Module,
    dep: &EntryField,
    refs: &mut Vec<Symbol>,
) -> String {
    if entry.is_forwarded(module, &dep.name) {
        if let Some((lib, ty)) = foreign_handle(&dep.target, module) {
            let handle = lib.types.iter().find(|t| t.name == ty);
            if let Some(raw) = handle.and_then(|h| handle_go_type(lib, h, module, refs)) {
                return raw;
            }
        }
    }
    push_field_type_symbols(&dep.target, module, refs);
    field_go_type_storage(&dep.target, module)
}

/// The argument a resolver call passes for one dependency: a forwarded
/// handle's local, or the settings field.
pub(super) fn dep_arg(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    dep: &str,
) -> String {
    if entry.is_forwarded(module, dep) {
        local_ident(dep)
    } else {
        format!("s.{}", entry_field_ident(entry, module, config, dep))
    }
}

/// The resolver call at a construction site, relative to column zero: bind
/// the value and the error, fail construction on the error, and store the
/// value (wrapped in the handle's adapter when the resolver returned the
/// library's own value). A forwarded handle is bound and never stored: its
/// consuming resolver reads the local. `args` are the already-spelled
/// arguments, in dependency order.
pub(super) fn call_site(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
    args: &[String],
) -> String {
    let local = local_ident(&field.name);
    let name = resolver_name(entry, field, multi);
    let mut out = format!(
        "{local}, err := {name}({})\nif err != nil {{\n\treturn nil, err\n}}\n",
        args.join(", ")
    );
    if entry.is_forwarded(module, &field.name) {
        return out;
    }
    let dest = format!(
        "s.{}",
        entry_field_ident(entry, module, config, &field.name)
    );
    match raw_handle(field, module) {
        Some((lib, ty)) => out.push_str(&format!(
            "{dest} = &{adapter}{{real: {local}}}\n",
            adapter = handle_adapter_ident(&lib.name, &ty),
        )),
        None => out.push_str(&format!("{dest} = {local}\n")),
    }
    out
}

/// The resolver call at a construction site with its arguments spelled from
/// the settings and the forwarded locals.
pub(super) fn call_site_from_settings(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
) -> String {
    let args: Vec<String> = call_deps(field)
        .iter()
        .map(|dep| dep_arg(entry, module, config, dep))
        .collect();
    call_site(entry, module, config, multi, field, &args)
}

/// Every resolver of the entry, in resolution order, each tagged with the
/// library it calls into. A field sourced from a handle method resolves
/// through the handle's generated interface, so its resolver belongs to the
/// handle's library.
pub(super) fn resolver_decls(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
) -> Vec<ResolverDecl> {
    entry
        .construction_split(module)
        .tail
        .iter()
        .filter_map(|step| match step {
            TailStep::Call(field) => free_resolver(entry, module, config, multi, field),
            TailStep::HandleCall(field) => handle_resolver(entry, module, config, multi, field),
            TailStep::Dependent(_) => None,
        })
        .collect()
}

/// The parameter list of a resolver: one `name Type` per dependency, in the
/// order the declaration reads them.
fn params(
    entry: &EntryModel<'_>,
    module: &Module,
    field: &EntryField,
    refs: &mut Vec<Symbol>,
) -> Vec<String> {
    call_deps(field)
        .iter()
        .filter_map(|dep| entry.fields.iter().find(|f| f.name == *dep).copied())
        .map(|dep| {
            format!(
                "{} {}",
                local_ident(&dep.name),
                dep_type(entry, module, dep, refs)
            )
        })
        .collect()
}

/// The resolver of a `= ns.fn(args)` field: the call, its `yields`-position
/// bindings, the declared sentinel-to-error mapping, and the `returns:`
/// projection (or the library's own value when the field is a handle, or
/// the bare result when the extern's return already is the logical type).
fn free_resolver(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
) -> Option<ResolverDecl> {
    let call = field.call.as_ref()?;
    let lib = find_lib(module, &call.ns)?;
    let decl = find_extern(lib, &call.func)?;
    let lang = go_lang(decl)?;
    let mut refs = Vec::new();
    let alias = import_lib(&mut refs, lib)?;
    if binds_ctx(lang) {
        refs.push(import("context", "context"));
    }
    let params = params(entry, module, field, &mut refs);
    let mut ref_expr = |path: &[String]| param_path_expr(config, path);
    let built = build_call(
        &mut refs,
        module,
        lib,
        lang,
        &Callee::Package(alias),
        &decl.params,
        &call.args,
        &field.name,
        BACKGROUND_CTX,
        &mut ref_expr,
    );
    let ret_ty = match raw_handle(field, module) {
        Some((handle_lib, ty)) => handle_lib
            .types
            .iter()
            .find(|t| t.name == ty)
            .and_then(|h| handle_go_type(handle_lib, h, module, &mut refs))
            .unwrap_or_else(|| field_go_type_storage(&field.target, module)),
        None => {
            push_field_type_symbols(&field.target, module, &mut refs);
            field_go_type_storage(&field.target, module)
        }
    };
    let mut body = String::new();
    if built.err_var.is_some() {
        body.push_str(&format!("\tvar zero {ret_ty}\n"));
    }
    body.push_str(&nest_go(&built.stmt));
    if let Some(err_var) = &built.err_var {
        body.push_str(&nest_go(&error_block(
            &mut refs,
            module,
            config,
            lib,
            &decl.errors,
            &format!("{}.{}", call.ns, call.func),
            err_var,
            &|expr| format!("return zero, {expr}"),
        )));
    }
    match &lang.returns {
        None => {
            let value = built
                .yields_vars
                .get("")
                .cloned()
                .or_else(|| built.yields_vars.values().next().cloned())
                .unwrap_or_else(|| "nil".to_string());
            body.push_str(&format!("\treturn {value}, nil\n"));
        }
        Some(returns) => {
            let (pre, expr) =
                returns_expr(module, config, returns, &built.yields_vars, &field.name);
            body.push_str(&nest_go(&pre));
            body.push_str(&format!("\treturn {expr}, nil\n"));
        }
    }
    let name = resolver_name(entry, field, multi);
    let text = format!(
        "// {name} resolves the {field} construction value through {ns}.{func}: it\n\
         // takes exactly what the declaration reads and returns the library's own\n\
         // value, so the constructor stores it and a generated test never calls it.\n\
         func {name}({params}) ({ret_ty}, error) {{\n{body}}}",
        field = field.name,
        ns = call.ns,
        func = call.func,
        params = params.join(", "),
    );
    Some(ResolverDecl {
        lib: lib.name.clone(),
        decl: Decl::raw_with(text, refs),
    })
}

/// The resolver of a `= .handle.method(args)` field: a call through the
/// handle's generated interface, whose methods are already signed in logical
/// types and already ran the `yields`/`returns`/`errors` projection behind
/// the adapter (or a test's fake), so the call is returned as it is. A
/// `ctx`-marked method receives the background context: construction is the
/// one-shot resolution the field exists for, with no caller deadline to
/// thread through.
fn handle_resolver(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
) -> Option<ResolverDecl> {
    let call = field.handle_call.as_ref()?;
    let head = call.recv.first()?;
    let recv_field = entry.fields.iter().find(|f| f.name == *head)?;
    let (lib, handle_ty) = foreign_handle(&recv_field.target, module)?;
    let handle = lib.types.iter().find(|t| t.name == handle_ty)?;
    let decl = handle.methods.iter().find(|m| m.name == call.method)?;
    let lang = go_lang(decl)?;
    let mut refs = Vec::new();
    if binds_ctx(lang) {
        refs.push(import("context", "context"));
    }
    let params = params(entry, module, field, &mut refs);
    let mut ref_expr = |path: &[String]| param_path_expr(config, path);
    let call_args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| {
            ext::call_arg_expr(
                &mut refs,
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
    push_field_type_symbols(&field.target, module, &mut refs);
    let ret_ty = field_go_type_storage(&field.target, module);
    let name = resolver_name(entry, field, multi);
    let text = format!(
        "// {name} resolves the {field} construction value from the {recv} handle's\n\
         // {method} method: the same call over the real handle or a test's fake.\n\
         func {name}({params}) ({ret_ty}, error) {{\n\
         \treturn {recv_expr}.{symbol}({args})\n\
         }}",
        field = field.name,
        recv = head,
        method = call.method,
        params = params.join(", "),
        recv_expr = param_path_expr(config, &call.recv),
        symbol = lang.symbol,
        args = call_args.join(", "),
    );
    Some(ResolverDecl {
        lib: lib.name.clone(),
        decl: Decl::raw_with(text, refs),
    })
}

/// One tab on every non-empty line, for a column-zero block moving into a
/// function body.
fn nest_go(block: &str) -> String {
    block
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                "\n".to_string()
            } else {
                format!("\t{l}\n")
            }
        })
        .collect()
}

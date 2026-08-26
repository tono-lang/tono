//! One named function per foreign construction, in a module of its own per
//! `ext` library (`ext_mathkit.rs` beside `client.rs`, a private `mod` of
//! the module's directory).
//!
//! A field constructed through a library (`formula = mathkit.from_formula(
//! .expr, .precision)`) is resolved by `resolve_formula(expr, precision)`:
//! the function takes exactly the settings the declaration reads (by
//! value, cloned at the call site the way every sibling read already is)
//! and returns the value the settings store, with the declared error mapping
//! inside. `new`/`build` is then the sequence of those calls in resolution
//! order; a handle forwarded into another call never enters the settings at
//! all (it is bound to a local and moved into the consuming resolver), so
//! the single-ownership rule is visible in the structure, not only in the
//! validation, and no slot is ever `take()`n.
//!
//! A field sourced from a handle method resolves through the same shape,
//! taking the handle's slot by reference (the method borrows the handle;
//! every later reader still finds it in place).

use crate::codegen::entries::plan::Emitter;
use crate::codegen::entries::{call_deps, TailStep};

use super::ext::{self, boxed_wrap, ok_pattern, ok_value};
use super::resolve::Resolver;
use super::resolve_call::{self, call_expr, error_match, find_extern, find_lib, find_rust_lang};
use super::*;

/// A resolver function, with the `ext` library it calls into (which decides
/// the module it lands in).
pub(super) struct ResolverDecl {
    pub(super) lib: String,
    pub(super) decl: Decl,
}

/// The name of the function resolving a field's foreign construction
/// (`resolve_formula`), entry-prefixed in a multi-entry module like every
/// other per-entry companion.
pub(super) fn resolver_name(entry: &EntryModel<'_>, field: &EntryField, multi: bool) -> String {
    if multi {
        snake(&format!("resolve_{}_{}", entry.name, field.name))
    } else {
        snake(&format!("resolve_{}", field.name))
    }
}

/// The Rust keywords and the constructor's own locals a field's snake_case
/// name must not shadow when it becomes a local or a parameter.
const RESERVED_LOCALS: [&str; 41] = [
    "as", "async", "await", "box", "break", "const", "continue", "crate", "dyn", "else", "enum",
    "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move",
    "mut", "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "type",
    "unsafe", "use", "where", "while", "s", "options", "recv",
];

/// The local a resolved value binds to (and the parameter name a resolver
/// takes it under): the field's snake_case name, suffixed when that name is
/// a keyword or one of the constructor's own locals.
pub(super) fn local_ident(name: &str) -> String {
    let ident = snake(name);
    if RESERVED_LOCALS.contains(&ident.as_str()) {
        format!("{ident}_value")
    } else {
        ident
    }
}

/// The local a forwarded field's resolved value binds to: `local_ident`,
/// suffixed when the field also carries `@with` — that field's own injected
/// value already destructures into a local of the bare name (an
/// `Option<T>`, the constructor's own parameter), so the *resolved* value
/// (the `T` either the injected value or the resolver call produced) needs a
/// name of its own to keep it from shadowing that parameter inside the
/// `if`/`else` that picks between them.
pub(super) fn forwarded_local_ident(field: &EntryField) -> String {
    let ident = local_ident(&field.name);
    if field.sources.iter().any(|s| matches!(s, Source::With)) {
        format!("{ident}_resolved")
    } else {
        ident
    }
}

/// Whether the resolver of a `= ns.fn(args)` or `= .handle.method(args)`
/// field is `async`: exactly when the extern's own `rust` binding is (the
/// call is awaited inside the resolver, so the call site awaits the
/// resolver).
fn call_is_async(field: &EntryField, module: &Module, entry: &EntryModel<'_>) -> bool {
    match &field.call {
        Some(call) => find_extern(find_lib(module, &call.ns), &call.func).is_async(LANG),
        None => field
            .handle_call
            .as_ref()
            .is_some_and(|call| ext::lookup(module, entry, call).decl.is_async(LANG)),
    }
}

/// A resolved value read off the settings for a resolver's argument: a
/// `Copy` primitive reads bare, everything else clones out of the slot (the
/// settings are read again later, the same "copy, don't move a sibling"
/// rule every other leaf follows).
fn clone_read(expr: &str, t: &Tref) -> String {
    if matches!(
        t,
        Tref::Prim(
            Prim::Bool
                | Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
                | Prim::Float
        )
    ) {
        expr.to_string()
    } else {
        format!("({expr}).clone()")
    }
}

/// The argument a resolver call passes for one dependency: a forwarded
/// handle's local (moved), the receiver slot by reference for a handle
/// method's receiver, or the settings field cloned.
fn dep_arg(r: &Resolver<'_, '_>, field: &EntryField, dep: &EntryField) -> String {
    if r.entry.is_forwarded(r.module, &dep.name) {
        return forwarded_local_ident(dep);
    }
    let read = r.path_expr(std::slice::from_ref(&dep.name));
    let is_receiver = field
        .handle_call
        .as_ref()
        .is_some_and(|c| c.recv.first() == Some(&dep.name));
    if is_receiver {
        format!("&{read}")
    } else {
        clone_read(&read, &dep.target)
    }
}

/// The resolver call at a construction site, relative to column zero: bind
/// the value (awaited when the resolver is `async`, `?` on its error) and
/// store it, wrapped into its `Option` slot when the settings hold it that
/// way. A forwarded handle is bound and never stored: its consuming
/// resolver takes the local.
pub(super) fn call_site(r: &Resolver<'_, '_>, field: &EntryField) -> String {
    let args: Vec<String> = call_deps(field)
        .iter()
        .filter_map(|dep| r.entry.fields.iter().find(|f| f.name == *dep).copied())
        .map(|dep| dep_arg(r, field, dep))
        .collect();
    let awaited = if call_is_async(field, r.module, r.entry) {
        ".await"
    } else {
        ""
    };
    let call = format!(
        "{}({}){awaited}?",
        resolver_name(r.entry, field, r.multi),
        args.join(", ")
    );
    if r.entry.is_forwarded(r.module, &field.name) {
        let ident = forwarded_local_ident(field);
        // A field also carrying `@with` shares its local with the injected
        // fallback (see `Resolver::with_assign`): both branches of that
        // `if`/`else` assign the same pre-declared binding, so this leaf
        // assigns too rather than shadowing it with a branch-scoped `let`.
        return if field.sources.iter().any(|s| matches!(s, Source::With)) {
            format!("{ident} = {call};")
        } else {
            format!("let {ident} = {call};")
        };
    }
    let dest = r.path_expr(std::slice::from_ref(&field.name));
    format!(
        "{dest} = {};",
        ext::wrap_stored(&field.target, r.module, &call)
    )
}

/// Every resolver of the entry, in resolution order, each tagged with the
/// library it calls into. A field sourced from a handle method resolves
/// through the handle, so its resolver belongs to the handle's library.
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
            TailStep::Call(field) => Some(free_resolver(entry, module, config, multi, field)),
            TailStep::HandleCall(field) => {
                Some(handle_resolver(entry, module, config, multi, field))
            }
            TailStep::Dependent(_) => None,
        })
        .collect()
}

/// A resolver rendering scope: the same leaf spellings the constructor uses,
/// with every sibling read spelled as the resolver's own parameter.
#[allow(clippy::too_many_arguments)]
fn scoped<'a, 'b>(
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    config: &'a CasingConfig,
    multi: bool,
    helpers: &'b mut Helpers,
    body: &'b mut String,
    refs: &'b mut Vec<Symbol>,
    resolve_fns: &'b mut Vec<Decl>,
) -> Resolver<'a, 'b> {
    Resolver {
        entry,
        module,
        config,
        helpers,
        arg_prefix: "",
        body,
        refs,
        resolve_fns,
        multi,
        param_scope: true,
    }
}

/// The parameter list of a resolver: one `name: Type` per dependency, in the
/// order the declaration reads them. A forwarded handle arrives by value
/// (moved from its local), a handle method's receiver by reference to its
/// slot, everything else by value.
fn params(entry: &EntryModel<'_>, module: &Module, field: &EntryField) -> Vec<String> {
    call_deps(field)
        .iter()
        .filter_map(|dep| entry.fields.iter().find(|f| f.name == *dep).copied())
        .map(|dep| {
            let is_receiver = field
                .handle_call
                .as_ref()
                .is_some_and(|c| c.recv.first() == Some(&dep.name));
            let ty = if is_receiver {
                format!("&{}", ext::settings_field_type(&dep.target, module))
            } else {
                ext::field_type(&dep.target, module)
            };
            format!("{}: {ty}", local_ident(&dep.name))
        })
        .collect()
}

/// The resolver of a `= ns.fn(args)` field: the awaited call, its
/// `yields`/`returns` projection (or the bare `Ok` value, boxed when the
/// settings hold the handle as a trait object), and its `errors:` mapping
/// onto the closed taxonomy.
fn free_resolver(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
) -> ResolverDecl {
    let call = field.call.as_ref().expect("a call step carries a call");
    let lib = find_lib(module, &call.ns);
    let decl = find_extern(lib, &call.func);
    let lang = find_rust_lang(decl);
    let mut helpers = Helpers::default();
    let mut body_sink = String::new();
    let mut refs = Vec::new();
    let mut resolve_fns = Vec::new();
    let mut r = scoped(
        entry,
        module,
        config,
        multi,
        &mut helpers,
        &mut body_sink,
        &mut refs,
        &mut resolve_fns,
    );
    let expr = call_expr(&mut r, call);
    let binding = ok_pattern(&lang.yields);
    let value = ok_value(&binding, lang.returns.as_ref(), true, true);
    let value = match boxed_wrap(&field.target, module) {
        Some(wrap) => format!("{wrap}({value})"),
        None => value,
    };
    let body = if lang.is_infallible() {
        format!("let {binding} = {expr};\nOk({value})")
    } else {
        format!(
            "match {expr} {{\n    Ok({binding}) => Ok({value}),\n    Err(e) => Err({mapped}),\n}}",
            mapped = error_match(module, lib, &call.ns, &call.func, &decl.errors),
        )
    };
    let effect = if decl.is_async(LANG) { "async " } else { "" };
    let name = resolver_name(entry, field, multi);
    let text = format!(
        "/// Resolves the `{field}` construction value through `{ns}.{func}`: it\n\
         /// takes exactly what the declaration reads and returns what the settings\n\
         /// store, so the constructor stores it and a generated test never calls it.\n\
         pub(crate) {effect}fn {name}({params}) -> Result<{ret}, TonoError> {{\n{body}\n}}",
        field = field.name,
        ns = call.ns,
        func = call.func,
        params = params(entry, module, field).join(", "),
        ret = ext::field_type(&field.target, module),
        body = indent(&body, 1),
    );
    ResolverDecl {
        lib: call.ns.clone(),
        decl: Decl::raw_with(text, refs),
    }
}

/// The resolver of a `= .handle.method(args)` field: the receiver's slot,
/// taken by reference and diagnosed when unset, the awaited call through it,
/// and the same `Ok`/`Err` shape a free call takes.
fn handle_resolver(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
    field: &EntryField,
) -> ResolverDecl {
    let call = field
        .handle_call
        .as_ref()
        .expect("a handle-call step carries a handle_call");
    let l = ext::lookup(module, entry, call);
    let mut helpers = Helpers::default();
    let mut body_sink = String::new();
    let mut refs = Vec::new();
    let mut resolve_fns = Vec::new();
    let mut r = scoped(
        entry,
        module,
        config,
        multi,
        &mut helpers,
        &mut body_sink,
        &mut refs,
        &mut resolve_fns,
    );
    let recv_name = call.recv.join(".");
    let call_name = format!("{recv_name}.{}", call.method);
    let scope = resolve_call::CallScope {
        lib: l.lib,
        params: l.params,
        entry_args: &call.args,
    };
    let args: Vec<String> = l
        .lang
        .call_args
        .iter()
        .map(|a| resolve_call::call_arg_expr(&mut r, &scope, a))
        .collect();
    let awaited = if l.decl.is_async(LANG) { ".await" } else { "" };
    let binding = ok_pattern(&l.lang.yields);
    let value = ok_value(&binding, l.lang.returns.as_ref(), true, true);
    let value = match boxed_wrap(&field.target, module) {
        Some(wrap) => format!("{wrap}({value})"),
        None => value,
    };
    let call_text = ext::method_call(module, l.lib, "recv", &l.lang.symbol, &args);
    // The outcome is bound before it is matched so the receiver borrow ends
    // with the call, not with whatever the matched arm goes on to write (a
    // sibling field of the same in-progress draft, for a call site still
    // inside `s`'s own scope).
    let settle = if l.lang.is_infallible() {
        format!("let {binding} = outcome;\nOk({value})")
    } else {
        format!(
            "match outcome {{\n    Ok({binding}) => Ok({value}),\n    Err(e) => Err({mapped}),\n}}",
            mapped = error_match(module, l.lib, &recv_name, &call.method, &l.decl.errors),
        )
    };
    let body = format!(
        "let recv = match {slot} {{\n\
         \x20   Some(v) => v,\n\
         \x20   None => {{\n\
         \x20       return Err(TonoError::Config(ConfigError {{ message: {miss:?}.to_string() }}));\n\
         \x20   }}\n\
         }};\n\
         let outcome = {call_text}{awaited};\n\
         {settle}",
        slot = local_ident(&call.recv[0]),
        miss = format!("{call_name}: the {recv_name} handle is not configured"),
    );
    let effect = if l.decl.is_async(LANG) { "async " } else { "" };
    let name = resolver_name(entry, field, multi);
    let text = format!(
        "/// Resolves the `{field}` construction value from the `{recv}` handle's\n\
         /// `{method}` method: the same call over whatever handle the slot holds.\n\
         pub(crate) {effect}fn {name}({params}) -> Result<{ret}, TonoError> {{\n{body}\n}}",
        field = field.name,
        recv = recv_name,
        method = call.method,
        params = params(entry, module, field).join(", "),
        ret = ext::field_type(&field.target, module),
        body = indent(&body, 1),
    );
    ResolverDecl {
        lib: l.lib.name.clone(),
        decl: Decl::raw_with(text, refs),
    }
}

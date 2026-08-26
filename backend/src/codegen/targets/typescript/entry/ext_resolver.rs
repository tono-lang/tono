//! One named function per foreign construction, in a module of its own per
//! `ext` library (`mathkit_ext.ts` beside `client.ts`).
//!
//! A field constructed through a library (`formula = mathkit.from_formula(
//! .expr, .precision)`) is resolved by `resolveFormula(expr, precision)`: the
//! function takes exactly the settings the declaration reads and returns the
//! library's own value, with the declared error mapping inside. `create`
//! is then the sequence of those calls in resolution order, and a generated
//! test builds its own sequence, putting a fake where a call's result would
//! go. The module is not named by the barrel and the package's `exports`
//! map ends at the barrel, so a consumer never reaches a resolver: the test
//! file (inside the package) is the only other caller.
//!
//! A handle's resolver narrows the library's value to the handle's generated
//! interface (the structural shape TypeScript already checks every use
//! against; a generic class instantiates against it rather than against a
//! spelling the library may never name), and a handle forwarded into
//! another call never enters the settings at all: it is handed straight to
//! the consuming resolver, typed by the same interface.

use std::collections::BTreeSet;

use super::ext_call::call_body;
use super::ext_handle_call::handle_call_body;
use super::ext_handle_iface::resolve_handle;
use super::{camel, field_camel, field_ts_type, foreign_handle, type_refs, Helpers, Names};
use crate::codegen::entries::{call_deps, EntryModel, TailStep};
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{EntryField, Module};

/// A resolver function, with the `ext` library it calls into (which decides
/// the module it lands in).
pub(super) struct ResolverDecl {
    pub(super) lib: String,
    pub(super) decl: Decl,
}

/// The name of the function resolving a field's foreign construction
/// (`resolveFormula`), entry-prefixed in a multi-entry module like every
/// other per-entry companion.
pub(super) fn resolver_name(n: &Names, field: &EntryField) -> String {
    camel(&format!("resolve_{}{}", n.op_prefix, field.name))
}

/// The JavaScript reserved words and the constructor's own locals a field's
/// camelCase name must not shadow when it becomes a local or a parameter.
const RESERVED_LOCALS: [&str; 45] = [
    "await",
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "let",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "yield",
    "s",
    "config",
    "options",
    "raw",
    "e",
    "c",
];

/// The local a resolved value binds to (and the parameter name a resolver
/// takes it under): the field's camelCase name, suffixed when that name is a
/// reserved word or one of the constructor's own locals.
pub(super) fn local_ident(name: &str) -> String {
    let ident = camel(name);
    if RESERVED_LOCALS.contains(&ident.as_str()) {
        format!("{ident}Value")
    } else {
        ident
    }
}

/// A sibling-field path read inside a resolver body: the head is the
/// parameter the resolver takes it under, every later segment a plain member
/// access into whatever that field resolved to.
pub(super) fn param_path_expr(
    _entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    path: &[String],
) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        if i == 0 {
            out.push_str(&local_ident(seg));
        } else {
            out.push('.');
            out.push_str(&field_camel(seg, config));
        }
    }
    out
}

/// The TypeScript type a resolver takes a dependency under: its storage
/// type (a handle as its generated interface, forwarded or stored, so the
/// same resolver runs over the real handle or a fake).
fn dep_type(module: &Module, dep: &EntryField, refs: &mut Vec<Symbol>) -> String {
    refs.extend(type_refs(&dep.target, module));
    field_ts_type(&dep.target, module)
}

/// The argument a resolver call passes for one dependency: a forwarded
/// handle's local, or the settings field.
pub(super) fn dep_arg(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    dep: &str,
) -> String {
    if entry.is_forwarded(module, dep) {
        local_ident(dep)
    } else {
        super::checks::field_path_expr(entry, config, std::slice::from_ref(&dep.to_string()), "s")
    }
}

/// The resolver call at a construction site, relative to column zero: bind
/// the awaited value and store it. A forwarded handle is bound and never
/// stored: its consuming resolver reads the local. `args` are the
/// already-spelled arguments, in dependency order.
pub(super) fn call_site(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    n: &Names,
    field: &EntryField,
    args: &[String],
) -> String {
    let local = local_ident(&field.name);
    let name = resolver_name(n, field);
    let mut out = format!("const {local} = await {name}({});\n", args.join(", "));
    if entry.is_forwarded(module, &field.name) {
        return out;
    }
    let dest =
        super::checks::field_path_expr(entry, config, std::slice::from_ref(&field.name), "s");
    out.push_str(&format!("{dest} = {local};\n"));
    out
}

/// The resolver call at a construction site with its arguments spelled from
/// the settings and the forwarded locals.
pub(super) fn call_site_from_settings(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    n: &Names,
    field: &EntryField,
) -> String {
    let args: Vec<String> = call_deps(field)
        .iter()
        .map(|dep| dep_arg(entry, module, config, dep))
        .collect();
    call_site(entry, module, config, n, field, &args)
}

/// Every resolver of the entry, in resolution order, each tagged with the
/// library it calls into. A field sourced from a handle method resolves
/// through the handle's generated interface, so its resolver belongs to the
/// handle's library.
pub(super) fn resolver_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut Helpers,
) -> Vec<ResolverDecl> {
    entry
        .construction_split(module)
        .tail
        .iter()
        .filter_map(|step| match step {
            TailStep::Call(field) => Some(free_resolver(entry, n, module, config, helpers, field)),
            TailStep::HandleCall(field) => {
                handle_resolver(entry, n, module, config, helpers, field)
            }
            TailStep::Dependent(_) => None,
        })
        .collect()
}

/// The parameter list of a resolver: one `name: Type` per dependency, in the
/// order the declaration reads them.
fn params(
    entry: &EntryModel<'_>,
    module: &Module,
    field: &EntryField,
    refs: &mut Vec<Symbol>,
) -> String {
    call_deps(field)
        .iter()
        .filter_map(|dep| entry.fields.iter().find(|f| f.name == *dep).copied())
        .map(|dep| {
            format!(
                "{}: {}",
                local_ident(&dep.name),
                dep_type(module, dep, refs)
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The resolver of a `= ns.fn(args)` field: the call, its `yields`/
/// `returns` projection (or the bare result narrowed to the handle's
/// interface, or passed through when the extern's return already is the
/// logical type), and its error boundary.
fn free_resolver(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut Helpers,
    field: &EntryField,
) -> ResolverDecl {
    let call = field.call.as_ref().expect("a call step carries a call");
    let mut refs = Vec::new();
    let mut sentinel_types = BTreeSet::new();
    let params = params(entry, module, field, &mut refs);
    refs.extend(type_refs(&field.target, module));
    let ret_ty = field_ts_type(&field.target, module);
    let body = call_body(
        entry,
        config,
        module,
        field,
        call,
        &mut refs,
        &mut sentinel_types,
        &param_path_expr,
    );
    helpers.ext_error_types.extend(sentinel_types);
    let name = resolver_name(n, field);
    let text = format!(
        "// {name} resolves the {field} construction value through {ns}.{func}: it\n\
         // takes exactly what the declaration reads and returns the library's own\n\
         // value, so the client stores it and a generated test never calls it.\n\
         export async function {name}({params}): Promise<{ret_ty}> {{\n{body}\n}}",
        field = field.name,
        ns = call.ns,
        func = call.func,
        body = indent_body(&body),
    );
    ResolverDecl {
        lib: call.ns.clone(),
        decl: Decl::raw_with(text, refs),
    }
}

/// The resolver of a `= .handle.method(args)` field: the call through the
/// handle's generated interface, taken as a parameter, so the same function
/// runs over the real handle or a test's fake; the projection and the
/// error boundary sit inside, exactly as an op's own `impl` body performs
/// them (TypeScript has no adapter behind the interface).
fn handle_resolver(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut Helpers,
    field: &EntryField,
) -> Option<ResolverDecl> {
    let call = field.handle_call.as_ref()?;
    let head = call.recv.first()?;
    let recv_field = entry.fields.iter().find(|f| f.name == *head)?;
    if !foreign_handle(&recv_field.target, module) {
        return None;
    }
    let crate::ir::Tref::Ref { id, .. } = &recv_field.target else {
        return None;
    };
    let (lib, _) = resolve_handle(id, module)?;
    let mut refs = Vec::new();
    let mut sentinel_types = BTreeSet::new();
    let params = params(entry, module, field, &mut refs);
    refs.extend(type_refs(&field.target, module));
    let ret_ty = field_ts_type(&field.target, module);
    let recv_expr = param_path_expr(entry, config, &call.recv);
    let body = handle_call_body(
        entry,
        config,
        module,
        field,
        call,
        &mut refs,
        &mut sentinel_types,
        &recv_expr,
        &param_path_expr,
    );
    helpers.ext_error_types.extend(sentinel_types);
    let name = resolver_name(n, field);
    let text = format!(
        "// {name} resolves the {field} construction value from the {recv} handle's\n\
         // {method} method: the same call over the real handle or a test's fake.\n\
         export async function {name}({params}): Promise<{ret_ty}> {{\n{body}\n}}",
        field = field.name,
        recv = head,
        method = call.method,
        body = indent_body(&body),
    );
    Some(ResolverDecl {
        lib: lib.name.clone(),
        decl: Decl::raw_with(text, refs),
    })
}

fn indent_body(body: &str) -> String {
    body.lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_string()
            } else {
                format!("  {line}\n")
            }
        })
        .collect::<String>()
        .trim_end_matches('\n')
        .to_string()
}

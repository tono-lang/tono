//! Go emission for the `ext`/`extern` FFI library block: importing and
//! calling a declared foreign library from generated Go.
//!
//! Three sites read this module: a field's own `= ns.fn(args)` construction
//! call ([`call_assign`], wired as `Resolver::call_assign`), the foreign
//! opaque-handle type a field or config member can carry ([`foreign_handle`],
//! [`handle_go_type`]), and an op's own `impl .field.method(args)` body
//! (`impl_op`, which reuses [`call_arg_expr`] and [`error_block`] here).
//!
//! Verification is the target compiler, not this emitter: every value this
//! module reads off a `yields` position, an `errors:` sentinel, or a foreign
//! handle's assumed exported name is spelled as a direct Go field/identifier
//! access against whatever the real call returns. A library that does not
//! match the declared shape fails `go build`, which is the model's intended
//! failure mode, not a generation-time crash here — every lookup below
//! degrades to a commented placeholder instead of panicking when the IR is
//! inconsistent (the frontend typechecks `ns`/`fn` resolution in the normal
//! path; this stays total defensively rather than trusting that blindly).

use std::collections::HashMap;

use crate::codegen::casing::CasingConfig;
use crate::codegen::entries::EntryModel;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, EntryField, ErrorBinding, ExtLib, ExternDecl,
    ExternLang, ExternParam, Module, OpaqueType, Prim, ReturnsLit, ReturnsValue, ShapeKind, Tref,
};

use super::resolve::Resolver;
use super::{
    camel, entry_field_ident, field_pascal, field_path_expr, go_type, import, literal, pascal,
    pattern_literal, push_type_symbols,
};

/// A sibling entry field's read expression off `s` (field construction
/// scope): the head honors `@rename(go)` unless it is itself a foreign
/// handle (unexported regardless), every later segment is a plain member
/// access into whatever that field resolved to.
pub(super) fn sibling_path_expr(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    path: &[String],
) -> String {
    let mut out = "s".to_string();
    for (i, seg) in path.iter().enumerate() {
        out.push('.');
        if i == 0 {
            out.push_str(&entry_field_ident(entry, module, config, seg));
        } else {
            out.push_str(&field_pascal(seg, config));
        }
    }
    out
}

/// A valid Go identifier derived from the `ext` block's own declared name,
/// not from the import path: non-identifier bytes become `_`, and a result
/// that cannot start an identifier gets a leading `_`. Used as the in-code
/// package selector every generated call through this lib uses, and always
/// spelled out as an explicit import alias (see `GoRules::render_import`),
/// because the path's last segment is not a reliable guess at the real
/// package's own declared name — a `/vN` module-version suffix (the module
/// path versions, the package's `package` clause does not) is the case that
/// motivated always aliasing, but any arbitrarily-named package has the same
/// problem.
pub(super) fn lib_ident(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    match cleaned.chars().next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => cleaned,
        Some(_) => format!("_{cleaned}"),
        None => "extlib".to_string(),
    }
}

pub(super) fn lib_go_path(lib: &ExtLib) -> Option<&str> {
    lib.langs
        .iter()
        .find(|l| l.lang == "go")
        .map(|l| l.path.as_str())
}

pub(super) fn find_lib<'a>(module: &'a Module, ns: &str) -> Option<&'a ExtLib> {
    module.ext_libs.iter().find(|l| l.name == ns)
}

/// A free function `extern` declared directly in the `ext` block, or a
/// method declared inside one of its opaque `type` handles.
pub(super) fn find_extern<'a>(lib: &'a ExtLib, name: &str) -> Option<&'a ExternDecl> {
    lib.externs.iter().find(|e| e.name == name).or_else(|| {
        lib.types
            .iter()
            .flat_map(|t| t.methods.iter())
            .find(|e| e.name == name)
    })
}

pub(super) fn go_lang(decl: &ExternDecl) -> Option<&ExternLang> {
    decl.langs.iter().find(|l| l.lang == "go")
}

/// Import the lib's Go package and return the selector every call through it
/// uses, or `None` when the `ext` block declares no Go module path (it may
/// bind other targets only).
pub(super) fn import_lib(refs: &mut Vec<Symbol>, lib: &ExtLib) -> Option<String> {
    let path = lib_go_path(lib)?;
    let ident = lib_ident(&lib.name);
    refs.push(import(&ident, path));
    Some(ident)
}

/// Whether a field/member's declared type is a foreign opaque handle from
/// the module's own `ext_libs` (`"{lib}#{type}"`), and if so, which lib and
/// type it names.
pub(super) fn foreign_handle<'a>(t: &Tref, module: &'a Module) -> Option<(&'a ExtLib, String)> {
    let Tref::Ref { id, .. } = t else {
        return None;
    };
    let (lib_name, ty) = id.split_once('#')?;
    let lib = module.ext_libs.iter().find(|l| l.name == lib_name)?;
    lib.types
        .iter()
        .any(|t| t.name == ty)
        .then(|| (lib, ty.to_string()))
}

/// The Go type spelling of a foreign opaque handle: a pointer to the real
/// package's exported type, assumed to share its identifier with the
/// handle's own canonical name Pascal-cased — the same casing convention
/// every other nominal type in this codegen follows. This is the one guess
/// the target compiler is expected to grade: a library whose real exported
/// identifier differs fails `go build`, which is the intended failure mode,
/// not a generation-time crash.
pub(super) fn handle_go_type(lib: &ExtLib, type_name: &str) -> Option<String> {
    lib_go_path(lib)?;
    Some(format!("*{}.{}", lib_ident(&lib.name), pascal(type_name)))
}

/// The import a foreign handle field's type needs, alongside its spelling.
pub(super) fn handle_symbol(lib: &ExtLib) -> Option<Symbol> {
    let path = lib_go_path(lib)?;
    Some(import(&lib_ident(&lib.name), path))
}

#[path = "ext_handle.rs"]
mod ext_handle;
pub(super) use ext_handle::{
    handle_adapter_decl, handle_adapter_ident, handle_iface_decl, handle_iface_type,
};

pub(super) fn literal_of_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "nil".to_string(),
        other => format!("{other:?}"),
    }
}

/// Render one `CallArg` as a Go expression. `ref_expr` resolves a bare
/// reference path (a `Param` first resolves through `params`/`entry_args`
/// into the caller's own actual argument, which is itself rendered
/// recursively — the two call sites this serves (a field's own construction
/// call and an op's `impl` method call) differ only in what a `Ref` reads,
/// so that is the one thing the caller supplies).
pub(super) fn call_arg_expr(
    refs: &mut Vec<Symbol>,
    lib: &ExtLib,
    arg: &CallArg,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    match arg {
        CallArg::Param(name) => {
            let idx = params.iter().position(|p| p.name == *name);
            match idx.and_then(|i| entry_args.get(i)) {
                Some(actual) => call_arg_expr(refs, lib, actual, params, entry_args, ref_expr),
                // Unreachable through `tono check`: an extern's own params
                // and its per-language `call:` args are arity-checked
                // against each other on the frontend.
                None => "nil".to_string(),
            }
        }
        CallArg::Ref(path) => ref_expr(path),
        CallArg::Lit(v) => literal_of_json(v),
        CallArg::List(items) => format!(
            "[]any{{{}}}",
            items
                .iter()
                .map(|a| call_arg_expr(refs, lib, a, params, entry_args, ref_expr))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Ctor(ctor) => ctor_expr(refs, lib, ctor, params, entry_args, ref_expr),
        // A nested extern call as a call argument (e.g. a request-signing
        // call read at a trait-argument site) is not exercised by the RFC
        // appendix; deferred rather than guessed at.
        CallArg::Call(_) => "nil /* nested extern-call argument: deferred */".to_string(),
    }
}

pub(super) fn ctor_expr(
    refs: &mut Vec<Symbol>,
    lib: &ExtLib,
    ctor: &CallCtor,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    let path = lib_go_path(lib);
    let Some(path) = path else {
        return "nil".to_string();
    };
    let ident = lib_ident(&lib.name);
    refs.push(import(&ident, path));
    let fields: Vec<String> = ctor
        .fields
        .iter()
        .map(|(name, value)| {
            let expr = call_arg_expr(refs, lib, value, params, entry_args, ref_expr);
            // Mirrors the foreign side's own field spelling verbatim, like
            // a declared foreign struct's fields: the ctor's field key is
            // written exactly as the caller declared it.
            format!("{name}: {expr}")
        })
        .collect();
    format!("{ident}.{}{{{}}}", pascal(&ctor.name), fields.join(", "))
}

pub(super) fn yields_path_expr(yields_vars: &HashMap<String, String>, path: &[String]) -> String {
    let head = path.first().cloned().unwrap_or_default();
    let mut expr = yields_vars
        .get(&head)
        .cloned()
        .unwrap_or_else(|| "nil".to_string());
    for seg in path.iter().skip(1) {
        expr.push('.');
        expr.push_str(seg);
    }
    expr
}

pub(super) fn member_type(module: &Module, target: &Tref, member: &str) -> Option<Tref> {
    let Tref::Ref { id, .. } = target else {
        return None;
    };
    let shape = module.shapes.iter().find(|s| s.id == *id)?;
    match &shape.kind {
        ShapeKind::Structure { members, .. } => members
            .iter()
            .find(|m| m.name == member)
            .map(|m| m.target.clone()),
        _ => None,
    }
}

/// The `returns: Type { field: value, ... }` projection: a preamble (hoisted
/// `match` variables, empty when every field is a bare reference) and the
/// struct-literal expression itself. `var_prefix` names the hoisted match
/// variables (the field's own name for a construction call, the op's for a
/// method call), so two calls in the same scope never collide.
pub(super) fn returns_expr(
    module: &Module,
    config: &CasingConfig,
    returns: &ReturnsLit,
    yields_vars: &HashMap<String, String>,
    var_prefix: &str,
) -> (String, String) {
    let ty = go_type(&returns.r#type);
    let mut pre = String::new();
    let mut parts: Vec<String> = Vec::new();
    for rf in &returns.fields {
        let ident = field_pascal(&rf.name, config);
        match &rf.value {
            ReturnsValue::Field(path) => {
                parts.push(format!("{ident}: {}", yields_path_expr(yields_vars, path)));
            }
            ReturnsValue::Select(select) => {
                let var = format!("{}{}", camel(var_prefix), pascal(&rf.name));
                let member_ty = member_type(module, &returns.r#type, &rf.name)
                    .unwrap_or(Tref::Prim(Prim::String));
                let subject = yields_path_expr(yields_vars, &select.subject);
                pre.push_str(&format!("var {var} {}\n", go_type(&member_ty)));
                pre.push_str(&format!("switch {subject} {{\n"));
                for arm in &select.arms {
                    let body = match &arm.value {
                        ArmValue::Lit(v) => literal(&member_ty, v),
                        ArmValue::Field(path) => yields_path_expr(yields_vars, path),
                        // No source chain exists inside a `returns:` match
                        // arm: only a literal or a yields-bound reference.
                        // Unreachable through `tono check`.
                        ArmValue::Sources(_) => "nil".to_string(),
                    };
                    match &arm.pattern {
                        Some(p) => pre
                            .push_str(&format!("case {}:\n\t{var} = {body}\n", pattern_literal(p))),
                        None => pre.push_str(&format!("default:\n\t{var} = {body}\n")),
                    }
                }
                pre.push_str("}\n");
                parts.push(format!("{ident}: {var}"));
            }
        }
    }
    (pre, format!("{ty}{{{}}}", parts.join(", ")))
}

/// The declared-error literal an `errors:` sentinel maps to: the shape's own
/// Go type, with its first required string-like member filled from the
/// wrapped Go error's message (the closest a bespoke sentinel gets to a
/// reason, matching the `Cause`/`Message` shape every other boundary wrap in
/// this target uses), or a bare zero value when no such member exists.
pub(super) fn declared_error_literal(
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    type_name: &str,
    err_var: &str,
) -> String {
    let ty = pascal(type_name);
    let message = module
        .shapes
        .iter()
        .find(|s| crate::codegen::entries::local_name(&s.id) == type_name)
        .and_then(|shape| match &shape.kind {
            ShapeKind::Structure { members, .. } => members
                .iter()
                .find(|m| m.required && matches!(m.target, Tref::Prim(Prim::String))),
            _ => None,
        })
        .map(|m| crate::codegen::conventions::field_ident(m, config, super::LANG));
    match message {
        Some(field) => format!("&{ty}{{{field}: {err_var}.Error()}}"),
        None => format!("&{ty}{{}}"),
    }
}

/// The error-handling block after a call assigns `err_var`: a declared
/// sentinel discriminates via `errors.Is` into its typed SDK error, in
/// declared order; anything else (including no `errors:` at all) becomes a
/// `ContractError` naming the extern. `ret` turns a built error expression
/// into the caller's own `return` statement (a construction call always
/// returns `nil, err`; an op method returns its own zero value).
#[allow(clippy::too_many_arguments)]
pub(super) fn error_block(
    refs: &mut Vec<Symbol>,
    module: &Module,
    config: &CasingConfig,
    lib: &ExtLib,
    errors: &[ErrorBinding],
    contract_name: &str,
    err_var: &str,
    ret: &dyn Fn(String) -> String,
) -> String {
    let contract = error_names().contract;
    let mut arms = String::new();
    if !errors.is_empty() {
        if let Some(alias) = import_lib(refs, lib) {
            refs.push(import("errors", "errors"));
            for binding in errors {
                let literal = declared_error_literal(module, config, &binding.r#type, err_var);
                arms.push_str(&format!(
                    "\tif errors.Is({err_var}, {alias}.{sentinel}) {{\n\t\t{ret_stmt}\n\t}}\n",
                    sentinel = binding.sentinel,
                    ret_stmt = ret(literal),
                ));
            }
        }
    }
    let fallback = ret(format!(
        "&{contract}{{ContractName: {contract_name:?}, Cause: {err_var}}}"
    ));
    format!("if {err_var} != nil {{\n{arms}\t{fallback}\n}}\n")
}

/// Whether any declared `yields` position is the reserved `error` sentinel
/// (an out-of-convention error position).
pub(super) fn has_error_position(lang: &ExternLang) -> bool {
    lang.yields.iter().any(|y| y.is_error)
}

/// The result of building and assigning an extern call's return values: the
/// Go statement(s) up to and including the call itself, the yields-bound
/// variable names (empty-string key for the no-`yields` case), and the
/// error variable's own name, `None` for an `infallible` call (the real Go
/// function has no error to check, so there is nothing to name).
struct CallResult {
    stmt: String,
    yields_vars: HashMap<String, String>,
    err_var: Option<String>,
}

/// Build the call expression and its LHS variable bindings; does not handle
/// error branching or `returns:` projection (the two call sites — a field's
/// own construction and an op's `impl` method call — diverge there).
/// `callee` is what `lang.symbol` is called on: the lib's package selector
/// for a free function, or the receiver's own read expression for a method.
/// `var_prefix` names every generated variable (the field's own name, or the
/// op's), so two calls sharing a scope never collide.
#[allow(clippy::too_many_arguments)]
fn build_call(
    refs: &mut Vec<Symbol>,
    lib: &ExtLib,
    lang: &ExternLang,
    callee: &str,
    decl_params: &[ExternParam],
    call_args_src: &[CallArg],
    var_prefix: &str,
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> CallResult {
    let call_args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(refs, lib, a, decl_params, call_args_src, ref_expr))
        .collect();
    let call_expr = format!("{callee}.{}({})", lang.symbol, call_args.join(", "));

    let prefix = camel(var_prefix);
    let has_error_pos = has_error_position(lang);
    let mut lhs: Vec<String> = Vec::new();
    let mut yields_vars: HashMap<String, String> = HashMap::new();
    let mut err_var: Option<String> = if lang.infallible {
        None
    } else {
        Some(format!("{prefix}Err"))
    };
    if lang.yields.is_empty() {
        // "sem yields: o retorno já é o tipo lógico" — the raw call result
        // is the target's own declared type, with no projection.
        let tmp = format!("{prefix}Result");
        lhs.push(tmp.clone());
        if let Some(err_var) = &err_var {
            lhs.push(err_var.clone());
        }
        yields_vars.insert(String::new(), tmp);
    } else if has_error_pos {
        for y in &lang.yields {
            if y.is_error {
                let v = format!("{prefix}{}", pascal(&y.name));
                err_var = Some(v.clone());
                lhs.push(v);
            } else {
                let v = format!("{prefix}{}", pascal(&y.name));
                lhs.push(v.clone());
                yields_vars.insert(y.name.clone(), v);
            }
        }
    } else {
        for y in &lang.yields {
            let v = format!("{prefix}{}", pascal(&y.name));
            lhs.push(v.clone());
            yields_vars.insert(y.name.clone(), v);
        }
        if let Some(err_var) = &err_var {
            lhs.push(err_var.clone());
        }
    }

    CallResult {
        stmt: format!("{} := {call_expr}\n", lhs.join(", ")),
        yields_vars,
        err_var,
    }
}

/// A field's own `= ns.fn(args)` construction call: the call itself, its
/// `yields`-position bindings (implicit trailing `error` unless an explicit
/// position is marked, matching each target's own idiomatic error-return
/// convention), the sentinel-to-error mapping, and the `returns:` projection
/// (or the bare call result when the extern's return already is the logical
/// type).
pub(super) fn call_assign(
    r: &mut Resolver<'_, '_>,
    field: &EntryField,
    call: &EntryCall,
    dest: &str,
) -> String {
    let Some(lib) = find_lib(r.module, &call.ns) else {
        return format!("// unresolved ext lib {:?}\n{dest} = nil", call.ns);
    };
    let Some(decl) = find_extern(lib, &call.func) else {
        return format!(
            "// unresolved extern {:?}.{:?}\n{dest} = nil",
            call.ns, call.func
        );
    };
    let Some(lang) = go_lang(decl) else {
        return format!(
            "// {:?}.{:?} declares no Go binding\n{dest} = nil",
            call.ns, call.func
        );
    };
    let Some(alias) = import_lib(r.refs, lib) else {
        return format!("// {:?} declares no Go module path\n{dest} = nil", call.ns);
    };

    let (entry, module, config) = (r.entry, r.module, r.config);
    let mut ref_expr = move |path: &[String]| sibling_path_expr(entry, module, config, path);
    let built = build_call(
        r.refs,
        lib,
        lang,
        &alias,
        &decl.params,
        &call.args,
        &field.name,
        &mut ref_expr,
    );

    let mut out = built.stmt;
    if let Some(err_var) = &built.err_var {
        out.push_str(&error_block(
            r.refs,
            r.module,
            r.config,
            lib,
            &lang.errors,
            &format!("{}.{}", call.ns, call.func),
            err_var,
            &|expr| format!("return nil, {expr}"),
        ));
    }

    match &lang.returns {
        None => {
            let value = built
                .yields_vars
                .get("")
                .cloned()
                .or_else(|| built.yields_vars.values().next().cloned())
                .unwrap_or_else(|| "nil".to_string());
            // The field's own static type is the generated interface, never
            // the real construction call's own concrete return type (its
            // methods return the foreign package's own types, not the
            // logical ones the interface declares), so the raw value is
            // wrapped in the matching adapter before it is stored.
            match foreign_handle(&field.target, r.module) {
                Some((handle_lib, type_name)) => out.push_str(&format!(
                    "{dest} = &{adapter}{{real: {value}}}\n",
                    adapter = handle_adapter_ident(&handle_lib.name, &type_name),
                )),
                None => out.push_str(&format!("{dest} = {value}\n")),
            }
        }
        Some(returns) => {
            let (pre, expr) =
                returns_expr(r.module, r.config, returns, &built.yields_vars, &field.name);
            out.push_str(&pre);
            out.push_str(&format!("{dest} = {expr}\n"));
        }
    }
    out
}

/// An op's own `impl .field.method(args)` body: a call into an entry
/// field's declared opaque-handle method, in place of the transport.
/// Shares the call/yields/errors/returns machinery with [`call_assign`]; the
/// receiver is a foreign handle field instead of an `ext` namespace, the
/// failure path returns the op's own zero value instead of `nil, err`, and a
/// `Ref` argument can read either an entry field or the op's own declared
/// parameter.
#[allow(clippy::too_many_arguments)]
pub(super) fn impl_call_body(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    op_name: &str,
    input_name: Option<&str>,
    call: &crate::ir::OpImplCall,
    ret_zero: &str,
    fail: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let Some(head) = call.recv.first() else {
        return "// impl call with no receiver\n".to_string();
    };
    let Some(field) = entry.fields.iter().find(|f| f.name == *head) else {
        return format!("// unresolved receiver field {head:?}\n");
    };
    let Some((lib, handle_ty)) = foreign_handle(&field.target, module) else {
        return format!("// {head:?} is not a foreign handle field\n");
    };
    let Some(handle) = lib.types.iter().find(|t| t.name == handle_ty) else {
        return format!("// unresolved handle type {handle_ty:?}\n");
    };
    let Some(decl) = handle.methods.iter().find(|m| m.name == call.method) else {
        return format!("// unresolved method {:?} on {handle_ty:?}\n", call.method);
    };
    let Some(lang) = go_lang(decl) else {
        return format!("// {:?} declares no Go binding\n", call.method);
    };
    // The receiver field's own static type is tono's generated interface
    // (see `field_go_type_storage`), never the real package directly: its
    // methods are already signed in logical/canonical types (the interface
    // is generated from exactly this `ExternDecl`), and whatever backs it
    // (the real adapter, or a hermetic test's fake) already ran the
    // yields/returns/errors projection this call site would otherwise repeat
    // against the wrong (foreign) shape. So the op's own body is a plain
    // interface call plus the generic `fail` wrap, nothing declared-error-
    // specific: that specificity already happened once, behind the
    // interface.
    let recv_expr = field_path_expr(entry, module, config, &call.recv, "c.settings");
    let mut ref_expr = move |path: &[String]| match path.split_first() {
        Some((head, _)) if Some(head.as_str()) == input_name => {
            if path.len() == 1 {
                "input".to_string()
            } else {
                let mut out = "input".to_string();
                for seg in &path[1..] {
                    out.push('.');
                    out.push_str(&field_pascal(seg, config));
                }
                out
            }
        }
        _ => field_path_expr(entry, module, config, path, "c.settings"),
    };
    let call_args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(refs, lib, a, &decl.params, &call.args, &mut ref_expr))
        .collect();
    let prefix = camel(op_name);
    let out_var = format!("{prefix}Out");
    let err_var = format!("{prefix}Err");
    format!(
        "{out_var}, {err_var} := {recv_expr}.{}({})\n\
         if {err_var} != nil {{\n\treturn {ret_zero}{}\n}}\n\
         return {out_var}, nil\n",
        lang.symbol,
        call_args.join(", "),
        fail(err_var.clone()),
    )
}

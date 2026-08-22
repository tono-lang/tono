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
use crate::codegen::foreign_spelling::{self, go_builtin};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang,
    ExternParam, ForeignLang, Module, OpaqueType, Prim, ReturnsLit, ReturnsValue, ShapeKind, Tref,
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
///
/// Every use of this function feeds a real foreign call's own argument list
/// (a field's own construction call, `build_call` in [`call_assign`]), never
/// a tono-facing position. So when the referenced field is itself a foreign
/// handle *this generator itself constructed* (`field.call` ran the real
/// construction call and stored its result through [`call_assign`]'s own
/// adapter wrap), the stored value is unwrapped back to the real value the
/// foreign function actually declares its parameter as: the adapter exists
/// for the tono/library boundary, not for one foreign call feeding another.
///
/// A handle field with no `call` (`@arg`/`@with`) holds whatever the caller
/// supplied to satisfy the interface, which is not guaranteed to be this
/// generator's own adapter type: unwrapping it the same way would be an
/// unchecked type assertion that panics at runtime on any other conforming
/// value (a hermetic test's fake, most obviously). Passing such a field into
/// another foreign call is rejected up front, at generation time, by
/// `entries::validate::injected_handle_forwarded_to_another_call` (naming
/// both fields and the call), so this function never actually runs against
/// that combination; the interface value is still read as-is here rather
/// than unwrapped, so a bypass of that gate (hand-fed IR, a future caller of
/// this function) fails `go build` instead of panicking.
pub(super) fn sibling_path_expr(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    path: &[String],
) -> String {
    let mut out = "s".to_string();
    let mut head_handle = None;
    for (i, seg) in path.iter().enumerate() {
        out.push('.');
        if i == 0 {
            out.push_str(&entry_field_ident(entry, module, config, seg));
            head_handle = entry
                .fields
                .iter()
                .find(|f| f.name == *seg && f.call.is_some())
                .and_then(|f| foreign_handle(&f.target, module));
        } else {
            out.push_str(&field_pascal(seg, config));
        }
    }
    // A handle is opaque (it declares methods, never members), so a
    // multi-segment path can never resolve through one: `tono check` rejects
    // `.handle.member` before this ever runs. Asserted rather than silently
    // falling through to the unwrapped spelling, which would hand the
    // adapter to whatever segment follows instead of failing loudly.
    debug_assert!(
        head_handle.is_none() || path.len() == 1,
        "a foreign handle field cannot have a member path: {path:?}"
    );
    if path.len() == 1 {
        if let Some((lib, type_name)) = head_handle {
            let adapter = handle_adapter_ident(&lib.name, &type_name);
            return format!("{out}.(*{adapter}).real");
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

/// Whether a word of a foreign spelling is one of this module's own
/// generated types (a tono shape rendered in the same Go package), which a
/// spelling can use without the package selector: `Source[AppSettings]`.
pub(super) fn is_local_type(module: &Module, word: &str) -> bool {
    module
        .shapes
        .iter()
        .any(|s| pascal(crate::codegen::entries::local_name(&s.id)) == word)
}

/// A foreign spelling with the library's identifiers qualified by its
/// package selector: `Calculator[float64]` becomes
/// `mathkit.Calculator[float64]`, `*Source[AppSettings]` becomes
/// `*settingskit.Source[AppSettings]`. Go builtins and the module's own
/// generated types stay bare; a head the author qualified stays as written.
pub(super) fn qualify(spelling: &str, alias: &str, module: &Module) -> String {
    foreign_spelling::qualify(
        spelling,
        &format!("{alias}."),
        &|w| go_builtin(w) || is_local_type(module, w),
        true,
    )
}

/// The Go type spelling of a foreign opaque handle: the storage type its
/// `go` block declares, verbatim, qualified by the package selector. The
/// block spells the whole thing (`Calculator[float64]` for an interface held
/// by value, `*Provider` for a concrete struct held by pointer, the
/// instantiation included), so nothing is derived from the handle's name.
/// Every position that spells the real foreign type (a `With*` setter's
/// parameter, the adapter's `real` field, a constructor's concrete argument)
/// goes through here. `None` when the lib declares no Go module path or
/// the handle no `go` block: there is no storage to spell, and
/// `validate_calls::handle_storage_declared` refuses the field before any
/// emitter reaches it.
pub(super) fn handle_go_type(
    lib: &ExtLib,
    handle: &OpaqueType,
    module: &Module,
    refs: &mut Vec<Symbol>,
) -> Option<String> {
    let alias = import_lib(refs, lib)?;
    let storage = handle.storage("go")?;
    Some(qualify(storage, &alias, module))
}

/// The import a foreign handle field's type needs, alongside its spelling.
pub(super) fn handle_symbol(lib: &ExtLib) -> Option<Symbol> {
    let path = lib_go_path(lib)?;
    Some(import(&lib_ident(&lib.name), path))
}

#[path = "ext_handle.rs"]
mod ext_handle;
pub(super) use ext_handle::{
    handle_adapter_decl, handle_adapter_ident, handle_call_assign, handle_iface_decl,
    handle_iface_type, method_signature,
};

#[path = "ext_render.rs"]
pub(super) mod ext_render;
pub(super) use ext_render::{call_arg_expr, coerce, error_block, has_error_position, returns_expr};
// declared_error_literal has no caller in this file itself: it is exported
// for ext_tests's own unit coverage of it, not for anything ext.rs calls.
#[allow(unused_imports)]
pub(super) use ext_render::declared_error_literal;

/// The result of building and assigning an extern call's return values: the
/// Go statement(s) up to and including the call itself, the yields-bound
/// variable names (empty-string key for the no-`yields` case), and the
/// error variable's own name.
struct CallResult {
    stmt: String,
    yields_vars: HashMap<String, String>,
    err_var: Option<String>,
}

/// What a binding's callee spelling is called on: the library's package
/// (a free function, `mathkit.FromConstant[float64]`: the spelling's own
/// identifiers are qualified by the selector) or a receiver expression (a
/// handle method, `a.real.Compute`: the spelling is the method name).
pub(super) enum Callee {
    Package(String),
    Receiver(String),
}

/// What a declared context position reads as where no caller context
/// exists (a field's construction-time call).
pub(super) const BACKGROUND_CTX: &str = "context.Background()";

/// Whether a binding declares a context position (`#(ctx context.Context)`):
/// the one thing that decides whether a generated method signature carries
/// `ctx context.Context`, and whether the `context` package is imported.
pub(super) fn binds_ctx(lang: &ExternLang) -> bool {
    lang.call_args
        .iter()
        .any(|a| matches!(a, CallArg::Foreign(_)))
}

/// Build the call expression and its LHS variable bindings; does not handle
/// error branching or `returns:` projection (the two call sites, a field's
/// own construction and an op's `impl` method call, diverge there).
/// `var_prefix` names every generated variable (the field's own name, or the
/// op's), so two calls sharing a scope never collide. `ctx_expr` is what a
/// declared context position reads as at this call site.
#[allow(clippy::too_many_arguments)]
fn build_call(
    refs: &mut Vec<Symbol>,
    module: &Module,
    lib: &ExtLib,
    lang: &ExternLang,
    callee: &Callee,
    decl_params: &[ExternParam],
    call_args_src: &[CallArg],
    var_prefix: &str,
    ctx_expr: &str,
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> CallResult {
    let call_args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| {
            call_arg_expr(
                refs,
                module,
                lib,
                a,
                decl_params,
                call_args_src,
                ctx_expr,
                ref_expr,
            )
        })
        .collect();
    let head = match callee {
        Callee::Package(alias) => qualify(&lang.symbol, alias, module),
        Callee::Receiver(recv) => format!("{recv}.{}", lang.symbol),
    };
    let call_expr = format!("{head}({})", call_args.join(", "));

    let prefix = camel(var_prefix);
    let has_error_pos = has_error_position(lang);
    let mut lhs: Vec<String> = Vec::new();
    let mut yields_vars: HashMap<String, String> = HashMap::new();
    let mut err_var: Option<String> = Some(format!("{prefix}Err"));
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
    // A field's own construction call has no caller context to thread
    // through: a declared context position reads the background context
    // for this one-shot resolution.
    if binds_ctx(lang) {
        r.refs.push(import("context", "context"));
    }
    let built = build_call(
        r.refs,
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

    let mut out = built.stmt;
    if let Some(err_var) = &built.err_var {
        out.push_str(&error_block(
            r.refs,
            r.module,
            r.config,
            lib,
            &decl.errors,
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
        .map(|a| {
            call_arg_expr(
                refs,
                module,
                lib,
                a,
                &decl.params,
                &call.args,
                "ctx",
                &mut ref_expr,
            )
        })
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

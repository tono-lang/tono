//! Rust emission for the two `ext`/`extern` constructs that ride the entry
//! surface itself: a field typed by a foreign opaque handle
//! ([`foreign_handle`], [`handle_rust_type`], [`field_type`]), and an
//! operation whose own body is `impl .field.method(args)`
//! ([`impl_call_body`]).
//!
//! A handle's Rust spelling is the owned concrete type the declaring crate
//! exports (`{crate}::{Type}`), never a generated wrapper — or, for an
//! `interface` handle, the crate's own trait held as `Box<dyn Trait>` (see
//! [`handle_rust_type`]); an owned value is what the caller of an
//! `@arg`/`@with` field actually has to pass either way. The one guess
//! here is the exported identifier — the handle's own canonical name,
//! Pascal-cased, the same convention every other nominal type in this
//! codegen follows. A crate whose real identifier differs fails `cargo
//! build`, which is this model's intended failure mode, not a
//! generation-time crash (the same contract `resolve_call` documents for a
//! free extern call's own symbol).
//!
//! Rust has no zero value for a foreign type, so the *stored* spelling of a
//! handle-typed slot ([`settings_field_type`]) wraps it in `Option`: the
//! construction draft starts every field at a zero before resolution
//! assigns it, and `None` is the only inhabitant of an arbitrary foreign
//! type's slot that is guaranteed to exist. The `Option` never reaches the
//! public surface — a constructor parameter, a `.with_*` argument and a
//! builder field all keep the bare owned type ([`field_type`]) — and the one
//! place the wrapper is read back ([`impl_call_body`]'s receiver) turns an
//! unset handle into a diagnosed `ConfigError` instead of a panic.
//!
//! The lookups here `expect`/`unwrap_or_else`-panic on a broken invariant
//! rather than degrading to a comment, matching `resolve_call`'s own style:
//! `entries::validate_entries` proves every target in the generation call
//! supports these constructs before either function runs, and the frontend
//! typechecks that a receiver field and its method resolve at all.

use std::fmt::Write as _;

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::entries::plan::Emitter;
use crate::codegen::entries::EntryModel;
use crate::codegen::foreign_spelling::{self, rust_builtin};
use crate::ir::{
    CallArg, EntryCall, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpImplCall,
    OpaqueType, ReturnsField, ReturnsLit, ReturnsValue, Tref, YieldsPos,
};

use super::resolve::Resolver;
use super::resolve_call::{
    call_arg_expr, error_match, find_extern, find_lib, find_rust_lang, json_literal, ok_assign,
    CallScope,
};
use super::transport::FieldCtx;
use super::{field_snake, rust_type, LANG};

/// The `ExtLib` and opaque type a `Tref` names, when it names a foreign
/// handle declared in one of this module's own `ext` blocks. A shape id is
/// always `"{module_name}#{local_name}"` (the frontend's one universal
/// convention, no exception for a type declared inside an `ext` block), so
/// only the local name after `#` identifies it here; the ext block it
/// belongs to can differ from the module's own name and is found by
/// searching every lib's own types, not by matching the id's prefix against
/// a lib name.
pub(super) fn foreign_handle<'a>(
    t: &Tref,
    module: &'a Module,
) -> Option<(&'a ExtLib, &'a OpaqueType)> {
    let Tref::Ref { id, .. } = t else {
        return None;
    };
    let ty = id.split_once('#').map_or(id.as_str(), |(_, ty)| ty);
    module
        .ext_libs
        .iter()
        .find_map(|lib| lib.types.iter().find(|t| t.name == ty).map(|t| (lib, t)))
}

/// The declaring crate's own identifier, as spelled at a call site: the
/// `rust` module path with `-` normalized to `_` (Cargo's own crate-name
/// rule), matching what `resolve_call::call_expr` spells for a free extern
/// call through the same lib.
fn crate_ident(lib: &ExtLib) -> Option<String> {
    lib.langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
}

/// A foreign spelling with the library's identifiers qualified by the
/// crate: `Box<dyn Calculator<f64>>` becomes `Box<dyn mathkit::Calculator<f64>>`,
/// `FormulaCalculator::parse` becomes `mathkit::FormulaCalculator::parse`,
/// `Memo<.reading>` becomes `mathkit::Memo<Reading>`. Rust keywords,
/// primitives and the prelude stay bare, a reference to one of the
/// module's own types renders as the type in scope through `types::*`,
/// and a head the author qualified stays as written. Every other word is
/// the library's, whatever the module generates under the same name.
pub(super) fn qualify(spelling: &str, krate: &str, module: &Module) -> String {
    foreign_spelling::qualify(
        spelling,
        &format!("{krate}::"),
        &|w| rust_builtin(w) || w == krate,
        false,
        &crate::codegen::entries::generated_type(module),
    )
}

/// The Rust type spelling of a foreign opaque handle: the storage type its
/// `rust` block declares, verbatim, qualified by the crate. The block spells
/// the whole thing (`Box<dyn Calculator<f64>>` for a trait the constructors
/// return concrete types of, `ConstantCalculator<f64>` for one held
/// concretely, the instantiation included), so nothing is derived from the
/// handle's name. `None` when the lib declares no `rust` module path or the
/// handle no `rust` block: there is no storage to spell, and
/// `validate_calls::handle_storage_declared` refuses the field before any
/// emitter reaches it.
pub(super) fn handle_rust_type(
    lib: &ExtLib,
    handle: &OpaqueType,
    module: &Module,
) -> Option<String> {
    let krate = crate_ident(lib)?;
    let storage = handle.storage("rust")?;
    Some(qualify(storage, &krate, module))
}

/// The wrap a freshly constructed value goes through on its way into a
/// handle slot stored behind a box (`Box<dyn Trait>`, `Arc<..>`): the
/// constructor yields one of the library's own concrete types and the slot
/// holds the trait object, so the value is boxed at the construction site.
/// `None` for a slot that holds the concrete type itself.
pub(super) fn boxed_wrap(t: &Tref, module: &Module) -> Option<&'static str> {
    foreign_handle(t, module)
        .and_then(|(lib, handle)| crate_ident(lib).and(handle.storage("rust")))
        .and_then(foreign_spelling::rust_boxed)
}

/// The public Rust spelling of an entry field's declared type: a foreign
/// handle's own owned concrete type, or the ordinary nominal/primitive
/// rendering for everything else. This is what a constructor parameter, a
/// `.with_*` argument and a builder field are typed with.
pub(super) fn field_type(t: &Tref, module: &Module) -> String {
    match foreign_handle(t, module).and_then(|(lib, handle)| handle_rust_type(lib, handle, module))
    {
        Some(ty) => ty,
        None => rust_type(t),
    }
}

/// The *stored* spelling of a construction slot (a `Settings` field, a
/// config struct member): [`field_type`], wrapped in `Option` for a foreign
/// handle because the draft the resolution assigns into has to start at some
/// value and a foreign type has no zero (see the module doc).
pub(super) fn settings_field_type(t: &Tref, module: &Module) -> String {
    match foreign_handle(t, module).and_then(|(lib, handle)| handle_rust_type(lib, handle, module))
    {
        Some(ty) => format!("Option<{ty}>"),
        None => rust_type(t),
    }
}

/// Whether a construction slot of this type is stored wrapped (see
/// [`settings_field_type`]), so a leaf assigning into it wraps its value.
pub(super) fn is_stored_wrapped(t: &Tref, module: &Module) -> bool {
    foreign_handle(t, module)
        .and_then(|(lib, handle)| handle_rust_type(lib, handle, module))
        .is_some()
}

/// A resolved value on its way into a construction slot, wrapped when that
/// slot is stored as an `Option` (see [`settings_field_type`]). The value is
/// assumed to already have the slot's public type (an injected handle comes
/// in as the `Box<dyn ...>` the parameter is typed with); a freshly
/// *constructed* value goes through [`wrap_constructed`] instead.
pub(super) fn wrap_stored(t: &Tref, module: &Module, expr: &str) -> String {
    if is_stored_wrapped(t, module) {
        format!("Some({expr})")
    } else {
        expr.to_string()
    }
}

/// A value a foreign constructor (or handle method) just yielded, on its way
/// into a construction slot: the target coerces it into the declared
/// storage, boxing it when the slot holds a trait object (see
/// [`boxed_wrap`]), then wraps it like every other stored value.
pub(super) fn wrap_constructed(t: &Tref, module: &Module, expr: &str) -> String {
    match boxed_wrap(t, module) {
        Some(wrap) => wrap_stored(t, module, &format!("{wrap}({expr})")),
        None => wrap_stored(t, module, expr),
    }
}

/// The declared handle method [`impl_call_body`] resolves against: the
/// receiver field's own handle type, the method's `ExternDecl` and its
/// `rust` language block.
struct Lookup<'a> {
    lib: &'a ExtLib,
    decl: &'a ExternDecl,
    lang: &'a ExternLang,
    params: &'a [ExternParam],
}

fn lookup<'a>(module: &'a Module, entry: &EntryModel<'_>, call: &OpImplCall) -> Lookup<'a> {
    let head = call.recv.first().unwrap_or_else(|| {
        panic!("an op's own impl call has no receiver (validate_entries should have rejected this)")
    });
    let field = entry
        .fields
        .iter()
        .find(|f| f.name == *head)
        .unwrap_or_else(|| {
            panic!("an op's own impl call names undeclared receiver field {head:?}")
        });
    let (lib, handle) = foreign_handle(&field.target, module).unwrap_or_else(|| {
        panic!("receiver field {head:?} of an op's own impl call is not a foreign handle")
    });
    let decl = handle
        .methods
        .iter()
        .find(|m| m.name == call.method)
        .unwrap_or_else(|| {
            panic!(
                "an op's own impl call names undeclared method {:?} on handle {:?}",
                call.method, handle.name
            )
        });
    let lang = find_rust_lang_method(decl, &call.method);
    Lookup {
        lib,
        decl,
        lang,
        params: &decl.params,
    }
}

fn find_rust_lang_method<'a>(decl: &'a ExternDecl, method: &str) -> &'a ExternLang {
    decl.langs
        .iter()
        .find(|l| l.lang == LANG)
        .unwrap_or_else(|| {
            panic!("handle method {method:?} has no rust binding (validate_entries should have rejected this)")
        })
}

/// What one call argument resolves against: the entry's own resolved
/// settings, or the operation's own declared input parameter.
struct ArgCtx<'a> {
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    config: &'a CasingConfig,
    input_name: Option<&'a str>,
    params: &'a [ExternParam],
    args: &'a [CallArg],
    lib: &'a ExtLib,
}

impl ArgCtx<'_> {
    /// A field path as a Rust expression: the op's own input parameter when
    /// the head names it, otherwise a read off the resolved settings. Every
    /// read is cloned — `self` is borrowed immutably by the method and the
    /// settings are read again by later calls, exactly the "copy, don't
    /// move" rule `resolve_call` already follows for a sibling field.
    fn ref_expr(&self, path: &[String]) -> String {
        match path.split_first() {
            Some((head, rest)) if Some(head.as_str()) == self.input_name => {
                let mut out = "input".to_string();
                for seg in rest {
                    let _ = write!(out, ".{}", field_snake(seg, self.config));
                }
                format!("({out}).clone()")
            }
            _ => {
                let ctx = FieldCtx {
                    entry: self.entry,
                    module: self.module,
                    config: self.config,
                    input: None,
                };
                format!("({}).clone()", ctx.access(path))
            }
        }
    }

    /// A bare `Param(name)` position: the caller's own argument for the
    /// logical parameter of that name, resolved positionally against the
    /// declared parameter list (the same substitution Go's and
    /// TypeScript's own handle-call emitters perform), falling back to a
    /// same-named field read when the position has no declared argument.
    fn param_expr(&self, name: &str) -> String {
        let at = self.params.iter().position(|p| p.name == name);
        match at.and_then(|i| self.args.get(i)) {
            Some(arg) => self.arg_expr(arg),
            None => self.ref_expr(&[name.to_string()]),
        }
    }

    fn arg_expr(&self, arg: &CallArg) -> String {
        match arg {
            CallArg::Param(name) => self.param_expr(name),
            // The parameter crosses under its own Rust spelling: the value
            // coerced as the spelling asks (see `resolve_call::coerce`).
            CallArg::ParamAs { name, spelling } => {
                let param_type = self
                    .params
                    .iter()
                    .find(|p| p.name == *name)
                    .map(|p| p.r#type.clone())
                    .unwrap_or(crate::ir::Tref::Prim(crate::ir::Prim::String));
                let expr = self.param_expr(name);
                super::resolve_call::coerce(self.module, self.lib, &param_type, spelling, &expr)
                    .unwrap_or_else(|e| {
                        panic!("{e}; validate_calls::param_spelling_coerces should have refused it")
                    })
            }
            // Rust binds no position of its own: `validate_calls::
            // foreign_position_binds` refuses a declared position here.
            CallArg::Foreign(sp) => panic!(
                "a declared position {sp:?} reached Rust codegen; validate_calls::foreign_position_binds should have refused it"
            ),
            CallArg::Ref(path) => self.ref_expr(path),
            CallArg::Lit(v) => json_literal(v),
            CallArg::List(items) => format!(
                "vec![{}]",
                items
                    .iter()
                    .map(|a| self.arg_expr(a))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            CallArg::Ctor(ctor) => {
                let krate = crate_ident(self.lib)
                    .expect("validate::call_resolves checked a rust module path exists");
                let values: Vec<String> = ctor.fields.values().map(|v| self.arg_expr(v)).collect();
                super::resolve_call::ctor_expr(self.module, self.lib, &krate, ctor, &values)
            }
            CallArg::Call(nested) => self.nested_call_expr(nested),
            CallArg::TypeRef(_) => panic!(
                "a class reference as a call argument reached Rust codegen; \
                 validate_calls::class_reference_renders should have rejected it first"
            ),
            // A bare foreign-symbol call nested inside a `call:` line's own
            // argument list, e.g. `WithPrecision(precision)`: no declared
            // extern to resolve against, so no yields/returns/errors
            // projection, no `.await` -- rendered as a plain synchronous,
            // infallible call, the shape the bench's own nested calls have.
            CallArg::SymbolCall(sc) => {
                let krate = crate_ident(self.lib)
                    .expect("validate::call_resolves checked a rust module path exists");
                format!(
                    "{}({})",
                    qualify(&sc.symbol, &krate, self.module),
                    sc.args
                        .iter()
                        .map(|a| self.arg_expr(a))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
    }

    /// A free extern call standing in an argument position: the same
    /// crate-qualified, awaited-unless-`sync` spelling
    /// `resolve_call::call_expr` emits, resolved here rather than through
    /// that module's construction-scope `Resolver` (which has no `self`/
    /// input-parameter scope to read a nested argument against).
    fn nested_call_expr(&self, call: &EntryCall) -> String {
        let lib = find_lib(self.module, &call.ns);
        let decl = find_extern(lib, &call.func);
        let lang = find_rust_lang(decl);
        let krate =
            crate_ident(lib).expect("validate::call_resolves checked a rust module path exists");
        let inner = ArgCtx {
            entry: self.entry,
            module: self.module,
            config: self.config,
            input_name: self.input_name,
            params: &decl.params,
            args: &call.args,
            lib,
        };
        let args: Vec<String> = lang.call_args.iter().map(|a| inner.arg_expr(a)).collect();
        format!(
            "{}({}){}",
            qualify(&lang.symbol, &krate, self.module),
            args.join(", "),
            if decl.is_async(LANG) { ".await" } else { "" }
        )
    }
}

/// `yields`' non-error positions bound out of `Ok(..)`: one binds bare, more
/// than one destructures the tuple the single native success value must be
/// (mirrors `resolve_call::ok_pattern`).
fn ok_pattern(yields: &[YieldsPos]) -> String {
    let positions: Vec<&YieldsPos> = yields.iter().filter(|y| !y.is_error).collect();
    match positions.as_slice() {
        [] => "v".to_string(),
        [one] => one.name.clone(),
        many => format!(
            "({})",
            many.iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The `Ok(..)` arm's own value: a declared `returns:` projects the foreign
/// binding field by field into the op's declared output, its absence hands
/// the binding straight back (the target compiler grades whether that value
/// really is the declared output), and an op with no output at all returns
/// unit.
fn ok_value(binding: &str, returns: Option<&ReturnsLit>, has_output: bool) -> String {
    if !has_output {
        return "()".to_string();
    }
    let Some(returns) = returns else {
        return binding.to_string();
    };
    let ty = match &returns.r#type {
        Tref::Ref { id, .. } => type_ident_from_id(id),
        // A `returns:` type is always a declared named shape (the checker
        // enforces this); a primitive/list/map target has no fields to
        // project into and never reaches this branch.
        _ => String::new(),
    };
    let fields: Vec<String> = returns
        .fields
        .iter()
        .map(|ReturnsField { name, value }| {
            let expr = match value {
                ReturnsValue::Field(path) => path.join("."),
                // A `returns:` match inside a handle method's own projection
                // has no field/env scope to resolve against at this call
                // site; the construction-scope spelling
                // (`resolve_call::select_expr`) is the only one that exists,
                // and it is not reachable from here. Reading the subject
                // straight through keeps the arm total and fails loudly at
                // the target compiler if the shape does not match.
                ReturnsValue::Select(select) => select.subject.join("."),
            };
            format!("{name}: {expr}")
        })
        .collect();
    format!("{ty} {{ {} }}", fields.join(", "))
}

/// Everything [`impl_call_body`] needs beyond the entry itself.
pub(super) struct ImplCall<'a> {
    pub entry: &'a EntryModel<'a>,
    pub module: &'a Module,
    pub config: &'a CasingConfig,
    pub call: &'a OpImplCall,
    pub input_name: Option<&'a str>,
    pub has_output: bool,
}

/// An op's own `impl .field.method(args)` body: the receiver read back off
/// the resolved settings, the declared call through it, the
/// `yields`/`returns` projection into the op's own output, and the
/// `errors:` sentinels mapped onto the same closed taxonomy a free extern
/// call maps onto (`resolve_call::error_match`).
///
/// Returns the body relative to column zero, paired with whether the
/// generated method must be `async`: the extern's own `sync` flag decides,
/// exactly as it does for a free call's `.await` in `resolve_call`.
pub(super) fn impl_call_body(c: &ImplCall<'_>) -> (String, bool) {
    let l = lookup(c.module, c.entry, c.call);
    let ctx = ArgCtx {
        entry: c.entry,
        module: c.module,
        config: c.config,
        input_name: c.input_name,
        params: l.params,
        args: &c.call.args,
        lib: l.lib,
    };
    let args: Vec<String> = l.lang.call_args.iter().map(|a| ctx.arg_expr(a)).collect();

    // A handle is opaque (it declares methods, never members), so the
    // receiver is always a single field: `tono check` rejects
    // `.handle.member.method(..)` before this runs.
    debug_assert!(
        c.call.recv.len() == 1,
        "a foreign handle receiver cannot have a member path: {:?}",
        c.call.recv
    );
    let field_ctx = FieldCtx {
        entry: c.entry,
        module: c.module,
        config: c.config,
        input: None,
    };
    let stored = field_ctx.access(&c.call.recv);
    let recv_name = c.call.recv.join(".");
    let call_name = format!("{recv_name}.{}", c.call.method);

    let awaited = if l.decl.is_async(LANG) { ".await" } else { "" };
    let binding = ok_pattern(&l.lang.yields);
    let value = ok_value(&binding, l.lang.returns.as_ref(), c.has_output);
    let mapped = error_match(c.module, l.lib, &recv_name, &c.call.method, &l.decl.errors);
    let call = method_call(c.module, l.lib, "recv", &l.lang.symbol, &args);

    // A declared signature with no error channel binds the value itself;
    // anything else is the `Result` the convention gives the call.
    let outcome = if l.lang.is_infallible() {
        format!("let {binding} = {call}{awaited};\nOk({value})")
    } else {
        format!(
            "match {call}{awaited} {{\n\
             \x20   Ok({binding}) => Ok({value}),\n\
             \x20   Err(e) => Err({mapped}),\n\
             }}"
        )
    };
    let body = format!(
        "let recv = match &{stored} {{\n\
         \x20   Some(v) => v,\n\
         \x20   None => {{\n\
         \x20       return Err(TonoError::Config(ConfigError {{ message: {miss:?}.to_string() }}));\n\
         \x20   }}\n\
         }};\n\
         {outcome}",
        miss = format!("{call_name}: the {recv_name} handle is not configured"),
    );
    (body, l.decl.is_async(LANG))
}

/// A field's own `= .field.method(args)` construction source: the receiver
/// read back off the in-progress settings draft `s` (the handle field
/// resolved earlier in the same constructor, an `Option` slot that is
/// diagnosed rather than unwrapped when unset), the declared call through
/// it, and the `yields`/`returns` projection assigned into `dest` with the
/// same `Ok`/`Err` shape a free extern call takes (`resolve_call::
/// call_assign`), the `errors:` sentinels mapped onto the same closed
/// taxonomy. The receiver is borrowed, never moved: reading a method
/// leaves the handle in its slot for every later reader (an op's own `impl`
/// body, another field), which is what lets this form coexist with them
/// where forwarding the handle into another call could not.
pub(super) fn handle_call_assign(
    r: &mut Resolver<'_, '_>,
    field: &crate::ir::EntryField,
    call: &OpImplCall,
    dest: &str,
) -> String {
    let l = lookup(r.module, r.entry, call);
    let recv_name = call.recv.join(".");
    let call_name = format!("{recv_name}.{}", call.method);
    let scope = CallScope {
        lib: l.lib,
        params: l.params,
        entry_args: &call.args,
        call_name: call_name.clone(),
    };
    let args: Vec<String> = l
        .lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(r, &scope, a))
        .collect();
    let stored = r.path_expr(&call.recv);
    let awaited = if l.decl.is_async(LANG) { ".await" } else { "" };
    let binding = ok_pattern(&l.lang.yields);
    let mapped = error_match(r.module, l.lib, &recv_name, &call.method, &l.decl.errors);
    let target = field.target.clone();
    let module = r.module;
    let store = move |expr: &str| wrap_constructed(&target, module, expr);
    let assign = ok_assign(dest, &binding, l.lang.returns.as_ref(), &store);
    // The outcome is bound before it is matched so the receiver borrow ends
    // with the call: the assignment below writes another field of the same
    // draft, which a still-live borrow of the receiver's slot would block.
    // A declared signature with no error channel is the value itself, so
    // the binding is the outcome and there is nothing to match.
    let settle = if l.lang.is_infallible() {
        format!("let {binding} = outcome;\n{assign}")
    } else {
        format!(
            "match outcome {{\n    Ok({binding}) => {{ {assign} }}\n    Err(e) => {{ return Err({mapped}); }}\n}}"
        )
    };
    format!(
        "let recv = match &{stored} {{\n\
         \x20   Some(v) => v,\n\
         \x20   None => {{\n\
         \x20       return Err(TonoError::Config(ConfigError {{ message: {miss:?}.to_string() }}));\n\
         \x20   }}\n\
         }};\n\
         let outcome = {call}{awaited};\n\
         {settle}",
        miss = format!("{call_name}: the {recv_name} handle is not configured"),
        call = method_call(r.module, l.lib, "recv", &l.lang.symbol, &args),
    )
}

/// A handle method call: `recv.method(args)` for a bare method name, or
/// the path form when the spelling is a path (`Calculator::compute`): the
/// method is called through its trait or type with the receiver first,
/// which is what brings a trait into scope on a concretely held handle.
fn method_call(module: &Module, lib: &ExtLib, recv: &str, symbol: &str, args: &[String]) -> String {
    if symbol.contains("::") {
        let krate = crate_ident(lib).unwrap_or_default();
        let mut all = vec![recv.to_string()];
        all.extend(args.iter().cloned());
        format!("{}({})", qualify(symbol, &krate, module), all.join(", "))
    } else {
        format!("{recv}.{symbol}({})", args.join(", "))
    }
}

#[cfg(test)]
#[path = "ext_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ext_source_tests.rs"]
mod source_tests;

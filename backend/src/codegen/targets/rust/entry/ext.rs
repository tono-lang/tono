//! Rust emission for the two `ext`/`extern` constructs that ride the entry
//! surface itself: a field typed by a foreign opaque handle
//! ([`foreign_handle`], [`handle_rust_type`], [`field_type`]), and an
//! operation whose own body is `impl .field.method(args)`
//! ([`impl_call_body`]).
//!
//! A handle's Rust spelling is the owned concrete type the declaring crate
//! exports (`{crate}::{Type}`), never a generated wrapper: Rust has no
//! interface value to hide the real type behind, and an owned handle is what
//! the caller of an `@arg`/`@with` field actually has to pass. The one guess
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
use crate::codegen::entries::EntryModel;
use crate::ir::{
    CallArg, CallCtor, EntryCall, ExtLib, ExternDecl, ExternLang, ExternParam, Module, OpImplCall,
    OpaqueType, ReturnsField, ReturnsLit, ReturnsValue, Tref, YieldsPos,
};

use super::resolve_call::{error_match, find_extern, find_lib, find_rust_lang, json_literal};
use super::transport::FieldCtx;
use super::{field_snake, pascal, rust_type, LANG};

/// The `ExtLib` and opaque type a `Tref` names, when it names a foreign
/// handle declared in one of this module's own `ext` blocks (its id is
/// `"{ext_name}#{type_name}"`, the same `#`-qualified shape id every other
/// reference uses).
pub(super) fn foreign_handle<'a>(
    t: &Tref,
    module: &'a Module,
) -> Option<(&'a ExtLib, &'a OpaqueType)> {
    let Tref::Ref { id, .. } = t else {
        return None;
    };
    let (lib_name, ty) = id.split_once('#')?;
    let lib = module.ext_libs.iter().find(|l| l.name == lib_name)?;
    lib.types.iter().find(|t| t.name == ty).map(|t| (lib, t))
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

/// The Rust type spelling of a foreign opaque handle: the owned concrete
/// type the declaring crate exports.
///
/// A generic handle (`handle.instance` set) spells the *foreign* name
/// instead of the handle's own tono name (two handles can instantiate the
/// same foreign generic differently, so they cannot share an identifier),
/// with the instantiation argument appended as a type argument through the
/// same `rust_type` every other declared type reaches codegen through.
///
/// `None` when the lib declares no `rust` module path at all: there is no
/// crate to qualify the type with, so the caller falls back to the ordinary
/// nominal spelling and lets the target compiler report the unresolved name.
pub(super) fn handle_rust_type(lib: &ExtLib, handle: &OpaqueType) -> Option<String> {
    let krate = crate_ident(lib)?;
    let base = match &handle.instance {
        Some(inst) => pascal(&inst.foreign_name),
        None => pascal(&handle.name),
    };
    Some(match &handle.instance {
        Some(inst) => format!("{krate}::{base}<{}>", rust_type(&inst.arg)),
        None => format!("{krate}::{base}"),
    })
}

/// The public Rust spelling of an entry field's declared type: a foreign
/// handle's own owned concrete type, or the ordinary nominal/primitive
/// rendering for everything else. This is what a constructor parameter, a
/// `.with_*` argument and a builder field are typed with.
pub(super) fn field_type(t: &Tref, module: &Module) -> String {
    match foreign_handle(t, module).and_then(|(lib, handle)| handle_rust_type(lib, handle)) {
        Some(ty) => ty,
        None => rust_type(t),
    }
}

/// The *stored* spelling of a construction slot (a `Settings` field, a
/// config struct member): [`field_type`], wrapped in `Option` for a foreign
/// handle because the draft the resolution assigns into has to start at some
/// value and a foreign type has no zero (see the module doc).
pub(super) fn settings_field_type(t: &Tref, module: &Module) -> String {
    match foreign_handle(t, module).and_then(|(lib, handle)| handle_rust_type(lib, handle)) {
        Some(ty) => format!("Option<{ty}>"),
        None => rust_type(t),
    }
}

/// Whether a construction slot of this type is stored wrapped (see
/// [`settings_field_type`]), so a leaf assigning into it wraps its value.
pub(super) fn is_stored_wrapped(t: &Tref, module: &Module) -> bool {
    foreign_handle(t, module)
        .and_then(|(lib, handle)| handle_rust_type(lib, handle))
        .is_some()
}

/// A resolved value on its way into a construction slot, wrapped when that
/// slot is stored as an `Option` (see [`settings_field_type`]).
pub(super) fn wrap_stored(t: &Tref, module: &Module, expr: &str) -> String {
    if is_stored_wrapped(t, module) {
        format!("Some({expr})")
    } else {
        expr.to_string()
    }
}

/// The declared handle method [`impl_call_body`] resolves against: the
/// receiver field's own handle type, the method's `ExternDecl` and its
/// `rust` language block.
struct Lookup<'a> {
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
    let (_lib, handle) = foreign_handle(&field.target, module).unwrap_or_else(|| {
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
            CallArg::Ctor(CallCtor { name, fields }) => {
                let rendered: Vec<String> = fields
                    .iter()
                    .map(|(field_name, value)| format!("{field_name}: {}", self.arg_expr(value)))
                    .collect();
                format!("{name} {{ {} }}", rendered.join(", "))
            }
            CallArg::Call(nested) => self.nested_call_expr(nested),
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
        };
        let args: Vec<String> = lang.call_args.iter().map(|a| inner.arg_expr(a)).collect();
        format!(
            "{krate}::{}({}){}",
            lang.symbol,
            args.join(", "),
            if lang.sync { "" } else { ".await" }
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

    let awaited = if l.lang.sync { "" } else { ".await" };
    let binding = ok_pattern(&l.lang.yields);
    let value = ok_value(&binding, l.lang.returns.as_ref(), c.has_output);
    let mapped = error_match(&recv_name, &c.call.method, &l.lang.errors);

    let body = format!(
        "let recv = match &{stored} {{\n\
         \x20   Some(v) => v,\n\
         \x20   None => {{\n\
         \x20       return Err(TonoError::Config(ConfigError {{ message: {miss:?}.to_string() }}));\n\
         \x20   }}\n\
         }};\n\
         match recv.{symbol}({args}){awaited} {{\n\
         \x20   Ok({binding}) => Ok({value}),\n\
         \x20   Err(e) => Err({mapped}),\n\
         }}",
        miss = format!("{call_name}: the {recv_name} handle is not configured"),
        symbol = l.lang.symbol,
        args = args.join(", "),
    );
    (body, !l.lang.sync)
}

#[cfg(test)]
#[path = "ext_tests.rs"]
mod tests;

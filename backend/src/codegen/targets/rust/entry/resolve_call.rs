//! The extern-call leaf: `field: T = ns.fn(args)` lowered to a Rust call
//! against the declared crate, its `yields`/`returns` projection, and its
//! `errors:` sentinel mapping. Split out from `resolve.rs` (a new leaf
//! entirely, not a moved one) to keep that file's leaf table from growing
//! past the file-size gate.
//!
//! `ns`/`fn` are guaranteed to resolve against a real `extern` carrying a
//! `lang: "rust"` block: `validate::call_resolves` checks this ahead of
//! every Rust generation call, so the lookups below are `expect`ed rather
//! than diagnosed here — a miss at this point is a validator gap, not an
//! authoring error.
//!
//! The call is always awaited: Rust is an async-lowering target, and an
//! arbitrary third-party symbol is exactly the "might block on I/O" case
//! that target already treats as async. `constructor.rs` gates the
//! surrounding `new`/`build` to `async fn` whenever an entry has one of
//! these fields.

use std::fmt::Write as _;

use super::ext;
use super::resolve::Resolver;
use super::*;
use crate::codegen::entries::plan::Emitter;
use crate::codegen::foreign_spelling;
use crate::ir::{
    CallArg, CallCtor, EntryCall, ExtLib, ExternDecl, ExternLang, ExternParam, ForeignLang, Tref,
};

pub(super) fn find_lib<'a>(module: &'a Module, ns: &str) -> &'a ExtLib {
    module
        .ext_libs
        .iter()
        .find(|l| l.name == ns)
        .expect("validate::call_resolves/wire_call_resolves checked this ext block exists")
}

pub(super) fn find_extern<'a>(lib: &'a ExtLib, func: &str) -> &'a ExternDecl {
    lib.externs
        .iter()
        .find(|e| e.name == func)
        .expect("validate::call_resolves/wire_call_resolves checked this extern exists")
}

/// The path a binding's callee is spelled at: the callee spelling with the
/// library's identifiers crate-qualified (`mathkit::from_constant`,
/// `mathkit::FormulaCalculator::parse` for a static method of a type).
pub(super) fn qualified_symbol(crate_ident: &str, lang: &ExternLang, module: &Module) -> String {
    ext::qualify(&lang.symbol, crate_ident, module)
}

/// The conversion a value spelled under its own Rust type goes through:
/// `Option<T>` wraps the value in `Some(..)`, `&T` (and `&str` for a
/// `String`) lends it, the type's own default spelling (a handle's declared
/// storage, or the ordinary mapping) passes as is. Anything else has no
/// conversion Rust can write, and
/// `validate_calls::param_spelling_coerces` refuses it before generation.
pub(super) fn coerce(
    module: &Module,
    lib: &ExtLib,
    ty: &Tref,
    spelling: &str,
    expr: &str,
) -> Result<String, String> {
    coerce_from(
        module,
        &crate_of(lib),
        &spelled_type(module, ty),
        spelling,
        expr,
    )
}

/// The same conversion for a struct literal spelled under its own Rust
/// type (`&Opts` lends the literal for the call, `Option<Opts>` wraps it):
/// the form's type is what its `rust` block declares, the spelling says
/// how the literal crosses. `validate_calls::foreign_forms_declared`
/// refuses a spelling with no conversion before generation.
pub(super) fn form_coerce(
    module: &Module,
    lib: &ExtLib,
    block: &ForeignLang,
    spelling: &str,
    literal: &str,
) -> Result<String, String> {
    let krate = crate_of(lib);
    let form_type = ext::qualify(&block.name, &krate, module);
    coerce_from(module, &krate, &form_type, spelling, literal)
}

fn crate_of(lib: &ExtLib) -> String {
    lib.langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
        .unwrap_or_default()
}

/// The conversion from `default`, the Rust type a value already has at the
/// boundary, into `spelling`, the type the binding asks for.
fn coerce_from(
    module: &Module,
    krate: &str,
    default: &str,
    spelling: &str,
    expr: &str,
) -> Result<String, String> {
    let qualified = ext::qualify(spelling, krate, module);
    if qualified == default {
        return Ok(expr.to_string());
    }
    if let Some(inner) = foreign_spelling::rust_option(spelling) {
        if ext::qualify(inner, krate, module) == default {
            return Ok(format!("Some({expr})"));
        }
    }
    // A borrowed spelling of the same type, or `&str` for a `String`:
    // the value is lent for the call.
    if let Some(inner) = spelling.strip_prefix('&') {
        let inner = inner.trim_start();
        if ext::qualify(inner, krate, module) == default || (inner == "str" && default == "String")
        {
            return Ok(format!("&{expr}"));
        }
    }
    Err(format!(
        "cannot pass a {default} as {spelling} in Rust: no conversion from {default} to {spelling}"
    ))
}

/// The Rust type a logical type already has at the boundary: a handle's
/// declared storage (or a `Vec` of them), else the ordinary mapping.
fn spelled_type(module: &Module, t: &Tref) -> String {
    match t {
        Tref::List(inner) => format!("Vec<{}>", spelled_type(module, inner)),
        _ => ext::field_type(t, module),
    }
}

/// A foreign struct literal: the form's own Rust type, from its `rust`
/// block, with each field's value converted when the block spells the field
/// under its own type. `values` are the already-rendered field values, in
/// the ctor's own (key-sorted) order. A form with no `rust` block does not
/// exist in Rust; `validate_calls::foreign_form_declared` refuses the
/// binding first.
pub(super) fn ctor_expr(
    module: &Module,
    lib: &ExtLib,
    krate: &str,
    ctor: &CallCtor,
    values: &[String],
) -> String {
    let form = lib.structs.iter().find(|s| s.name == ctor.name);
    let Some(block) = form.and_then(|f| f.lang("rust")) else {
        panic!(
            "foreign struct {:?} declares no rust block; validate_calls::foreign_form_declared should have refused it",
            ctor.name
        );
    };
    let rendered: Vec<String> = ctor
        .fields
        .keys()
        .zip(values)
        .map(|(field_name, expr)| {
            let declared = form.and_then(|f| f.fields.iter().find(|ff| ff.name == *field_name));
            let expr = match (block.fields.get(field_name), declared) {
                (Some(spelling), Some(ff)) => coerce(module, lib, &ff.r#type, spelling, expr)
                    .unwrap_or_else(|e| {
                        panic!("{e}; validate_calls::field_spelling_coerces should have refused it")
                    }),
                _ => expr.clone(),
            };
            format!("{field_name}: {expr}")
        })
        .collect();
    let literal = format!(
        "{} {{ {} }}",
        ext::qualify(&block.name, krate, module),
        rendered.join(", ")
    );
    match &ctor.spelling {
        None => literal,
        Some(spelling) => form_coerce(module, lib, block, spelling, &literal).unwrap_or_else(|e| {
            panic!("{e}; validate_calls::foreign_forms_declared should have refused it")
        }),
    }
}

pub(super) fn find_rust_lang(decl: &ExternDecl) -> &ExternLang {
    decl.langs
        .iter()
        .find(|l| l.lang == "rust")
        .expect("validate::call_resolves/wire_call_resolves checked a rust block exists")
}

/// A `@default`-shaped best-effort Rust literal for a raw JSON call
/// argument: exact typed rendering needs the logical parameter's declared
/// type positionally matched against `call_args`, which is not always
/// possible (a `Ctor` field's `Lit` has no positional `ExternParam` at all),
/// so this stays intentionally naive rather than half-implementing the
/// general `literal()` machinery for a shape it cannot always see.
pub(super) fn json_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}.to_string()"),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "Default::default()".to_string(),
        _ => format!("serde_json::json!({v})"),
    }
}

/// The call one argument is rendered for: the declaring library (a `Ctor`
/// names one of its structs), the extern's declared parameter list and the
/// caller's own actual arguments (the entry field's `call(..)`), which a
/// `Param` position resolves through, and the `ns.fn` name a diagnostic
/// names.
pub(super) struct CallScope<'a> {
    pub lib: &'a ExtLib,
    pub params: &'a [ExternParam],
    pub entry_args: &'a [CallArg],
}

/// One `CallArg` as a Rust expression, in the target's own call syntax.
pub(super) fn call_arg_expr(
    r: &mut Resolver<'_, '_>,
    scope: &CallScope<'_>,
    arg: &CallArg,
) -> String {
    match arg {
        // A `call:` argument written inside the language block that names
        // a logical parameter arrives as a bare `Param` (the frontend does
        // not pre-resolve it to the field the caller passed), so it is
        // substituted positionally: the caller's own actual argument at the
        // parameter's declared position, rendered recursively. The same
        // substitution Go's and TypeScript's own emitters perform.
        CallArg::Param(name) => {
            let actual = scope
                .params
                .iter()
                .position(|p| p.name == *name)
                .and_then(|i| scope.entry_args.get(i))
                .unwrap_or_else(|| {
                    // The frontend arity-checks an extern's `call:` args
                    // against its declared parameters and the caller's args
                    // against the same list, so a `Param` with no actual
                    // argument at its position is a validator gap, never an
                    // authoring error this leaf should paper over.
                    panic!("extern param {name:?} has no argument at its position")
                });
            call_arg_expr(r, scope, actual)
        }
        // A handle-typed argument is always a forwarded one here:
        // `entries::validate_ownership` rejects a spec whose forwarded
        // handle has a second reader (an op receiver, another call) before
        // any target emits, so a handle reaching a call argument at all
        // means this is its one consumer. The resolver's own parameter for
        // it already holds the raw owned value (see `ext_resolver::
        // dep_type`/`dep_arg`), so it is moved here, not cloned — a handle
        // is not `Clone` in general (a connection, a pool, a provider
        // typically is not).
        CallArg::Ref(path) if ext::is_stored_wrapped(&r.path_type(path), r.module) => {
            r.path_expr(path)
        }
        // The parameter crosses under its own Rust spelling: the caller's
        // actual argument, converted as the spelling asks.
        CallArg::ParamAs { name, spelling } => {
            let idx = scope.params.iter().position(|p| p.name == *name);
            let (Some(param), Some(actual)) = (
                idx.map(|i| &scope.params[i]),
                idx.and_then(|i| scope.entry_args.get(i)),
            ) else {
                panic!("extern param {name:?} has no argument at its position")
            };
            let expr = call_arg_expr(r, scope, actual);
            coerce(r.module, scope.lib, &param.r#type, spelling, &expr).unwrap_or_else(|e| {
                panic!("{e}; validate_calls::param_spelling_coerces should have refused it")
            })
        }
        // Rust binds no position of its own: `validate_calls::
        // foreign_position_binds` refuses a declared position here.
        CallArg::Foreign(sp) => panic!(
            "a declared position {sp:?} reached Rust codegen; validate_calls::foreign_position_binds should have refused it"
        ),
        // Cloned, not moved: `s` (the composed settings struct) is still
        // read later (the frozen `ClientOptions`, `Client { settings: s,
        // .. }`), the same "copy, don't move a sibling" rule every other
        // leaf reading a resolved field already follows.
        CallArg::Ref(path) => format!("({}).clone()", r.path_expr(path)),
        CallArg::Lit(v) => json_literal(v),
        CallArg::List(items) => {
            let rendered: Vec<String> = items.iter().map(|a| call_arg_expr(r, scope, a)).collect();
            format!("vec![{}]", rendered.join(", "))
        }
        CallArg::Ctor(ctor) => {
            let krate = scope
                .lib
                .langs
                .iter()
                .find(|l| l.lang == "rust")
                .map(|l| l.path.replace('-', "_"))
                .expect("validate::call_resolves checked a rust module path exists");
            let values: Vec<String> = ctor
                .fields
                .values()
                .map(|v| call_arg_expr(r, scope, v))
                .collect();
            ctor_expr(r.module, scope.lib, &krate, ctor, &values)
        }
        CallArg::Call(nested) => call_expr(r, nested),
        // Rust has no type as a value to pass; `validate_calls` refuses the
        // binding by name before generation reaches here.
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
            // The symbol lives in the same crate as the enclosing `call:`,
            // so it is crate-qualified the same way `call_expr` qualifies a
            // declared extern's own symbol.
            let crate_ident = scope
                .lib
                .langs
                .iter()
                .find(|l| l.lang == "rust")
                .map(|l| l.path.replace('-', "_"))
                .expect("validate::call_resolves checked a rust module path exists");
            let rendered: Vec<String> =
                sc.args.iter().map(|a| call_arg_expr(r, scope, a)).collect();
            format!(
                "{}({})",
                ext::qualify(&sc.symbol, &crate_ident, r.module),
                rendered.join(", ")
            )
        }
    }
}

/// The bare call expression (crate-qualified symbol, args, `.await`), reused
/// both as a top-level field's assignment source and as a nested argument's
/// own value.
pub(super) fn call_expr(r: &mut Resolver<'_, '_>, call: &EntryCall) -> String {
    let lib = find_lib(r.module, &call.ns);
    let decl = find_extern(lib, &call.func);
    let lang = find_rust_lang(decl);
    // The crate is referenced fully qualified at the call site rather than
    // through a separate `use`: an external crate name is already in scope
    // by declaration (no glob/ambiguity risk), and this sidesteps having to
    // invent a `Symbol` shape for a dependency that lives outside the
    // generated SDK's own module tree.
    let crate_ident = lib
        .langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
        .expect("validate::call_resolves checked a rust module path exists");
    let scope = CallScope {
        lib,
        params: &decl.params,
        entry_args: &call.args,
    };
    let args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(r, &scope, a))
        .collect();
    let awaited = if decl.is_async("rust") { ".await" } else { "" };
    format!(
        "{}({}){awaited}",
        qualified_symbol(&crate_ident, lang, r.module),
        args.join(", ")
    )
}

/// The declared errors inside the `Err(e)` arm: each error the op lists
/// (`@errors`, in declared order) is recognized by the pattern its own
/// `rust` block spells (`Error::Parse`, matched as `Error::Parse { .. }` so
/// a unit, tuple or struct variant all fit), its message read from the
/// source the block maps `message` to (`to_string()` by default); anything
/// else (including an error with no `rust` block) is a `ContractError`
/// naming the extern, the same "declaration is a hypothesis the target
/// build already enforces, so an unmapped failure names its origin" idiom
/// `impl_op` uses for a bespoke operation.
pub(super) fn error_match(
    module: &Module,
    lib: &ExtLib,
    ns: &str,
    func: &str,
    errors: &[String],
) -> String {
    let contract_name = format!("{ns}.{func}");
    let fallback = format!(
        "TonoError::Contract(ContractError {{ contract_name: {contract_name:?}.to_string(), cause: e.to_string().into() }})"
    );
    let krate = lib
        .langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
        .unwrap_or_default();
    let bindings: Vec<ForeignLang> = errors
        .iter()
        .filter_map(|id| ForeignLang::of_error(module, id, "rust"))
        .collect();
    // No declared sentinel leaves a single wildcard arm, which clippy flags
    // as a match on nothing (`match_single_binding`): spell the fallback
    // directly rather than routing it through a match with one arm.
    if bindings.is_empty() {
        return fallback;
    }
    let mut arms = String::new();
    for fl in &bindings {
        let pattern = ext::qualify(&fl.name, &krate, module);
        let message = fl
            .fields
            .get("message")
            .map(|source| format!("e.{source}"))
            .unwrap_or_else(|| "e.to_string()".to_string());
        let _ = write!(
            arms,
            "e if matches!(e, {pattern} {{ .. }}) => TonoError::Config(ConfigError {{ message: {message} }}), ",
        );
    }
    format!("match e {{ {arms}_ => {fallback}, }}")
}

#[cfg(test)]
#[path = "resolve_call_tests.rs"]
mod tests;

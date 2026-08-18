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
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, ErrorBinding, ExtLib, ExternDecl, ExternLang,
    ExternParam, ReturnsField, ReturnsLit, ReturnsValue, Select, YieldsPos,
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
struct CallScope<'a> {
    lib: &'a ExtLib,
    params: &'a [ExternParam],
    entry_args: &'a [CallArg],
    call_name: String,
}

/// One `CallArg` as a Rust expression, in the target's own call syntax.
fn call_arg_expr(r: &mut Resolver<'_, '_>, scope: &CallScope<'_>, arg: &CallArg) -> String {
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
        // A foreign handle stored in its `Option` slot is moved out (a
        // handle is not `Clone` in general: a connection, a pool, a
        // provider), not cloned. `s` is a local the constructor owns, so
        // `take()` on its field is sound while `s` is still read later. The
        // slot is left unset, so nothing else may read the handle after
        // this: `entries::validate_ownership` rejects a spec whose
        // forwarded handle has a second reader (an op receiver, another
        // call) before any target emits, which is what makes the move
        // total here. An unset slot at this point can only mean the
        // handle's own resolution was skipped upstream, so it is diagnosed
        // the same way the op receiver read diagnoses it instead of
        // unwrapped.
        CallArg::Ref(path) if ext::is_stored_wrapped(&r.path_type(path), r.module) => {
            let miss = format!(
                "{}: the {} handle is not configured",
                scope.call_name,
                path.join(".")
            );
            format!(
                "match {}.take() {{ Some(v) => v, None => {{ return Err(TonoError::Config(ConfigError {{ message: {miss:?}.to_string() }})); }} }}",
                r.path_expr(path)
            )
        }
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
        CallArg::Ctor(CallCtor { name, fields }) => {
            scope
                .lib
                .structs
                .iter()
                .find(|s| &s.name == name)
                .expect("checker rejects a ctor naming an undeclared foreign struct");
            let rendered: Vec<String> = fields
                .iter()
                .map(|(field_name, value)| {
                    format!("{field_name}: {}", call_arg_expr(r, scope, value))
                })
                .collect();
            format!("{name} {{ {} }}", rendered.join(", "))
        }
        CallArg::Call(nested) => call_expr(r, nested),
    }
}

/// The bare call expression (crate-qualified symbol, args, `.await`), reused
/// both as a top-level field's assignment source and as a nested argument's
/// own value.
fn call_expr(r: &mut Resolver<'_, '_>, call: &EntryCall) -> String {
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
        call_name: format!("{}.{}", call.ns, call.func),
    };
    let args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(r, &scope, a))
        .collect();
    let awaited = if lang.sync { "" } else { ".await" };
    format!(
        "{crate_ident}::{}({}){awaited}",
        lang.symbol,
        args.join(", ")
    )
}

/// A `returns:` field's source expression, read off the `yields` binding
/// named in `ReturnsValue::Field`'s head segment (the remaining segments are
/// the foreign shape's own field names, verbatim: no casing engine touches a
/// declared-foreign form).
fn returns_value_expr(value: &ReturnsValue) -> String {
    match value {
        ReturnsValue::Field(path) => path.join("."),
        ReturnsValue::Select(select) => select_expr(select),
    }
}

/// A `match .subject { ... }` selection inside a `returns:` field, spelled
/// as a standalone expression (not the shared `Stmt` tree the rest of the
/// plan renders through, since this text is composed directly): the same
/// Display-string comparison the plan's own switch leaves use, so one form
/// covers a string, a numeric, or an open-enum subject alike.
fn select_expr(select: &Select) -> String {
    let subject = select.subject.join(".");
    let mut arms = String::new();
    for arm in &select.arms {
        let value = match &arm.value {
            ArmValue::Field(path) => path.join("."),
            ArmValue::Lit(v) => json_literal(v),
            // A nested source stack inside a returns-projection match arm
            // has no field/env context to resolve against here (this text
            // is composed outside the shared resolution plan); falling back
            // to the subject itself keeps the arm total without inventing a
            // resolution the grammar does not give this leaf enough to spell.
            ArmValue::Sources(_) => subject.clone(),
            // A `returns:` match arm has no entry-field subject to narrow
            // here (only the foreign yields binding); falling back to the
            // subject text keeps the arm total, same as `Sources` above.
            ArmValue::Subject => subject.clone(),
        };
        match &arm.pattern {
            // A match arm's pattern position takes a bare literal, not an
            // expression: `json_literal`'s `"x".to_string()` (built for a
            // value position) is not valid pattern syntax, so this uses the
            // same bare-literal spelling `pattern_literal` already gives the
            // shared plan's own switch leaves.
            Some(pat) => {
                let _ = write!(arms, "{} => {value}, ", pattern_literal(pat));
            }
            None => {
                let _ = write!(arms, "_ => {value}, ");
            }
        }
    }
    if select.arms.iter().all(|a| a.pattern.is_some()) {
        let _ = write!(arms, "_ => {subject}.clone(), ");
    }
    format!("match ({subject}).to_string().as_str() {{ {arms}}}")
}

/// The `errors:` mapping inside the `Err(e)` arm: a declared sentinel names
/// a diagnosed `ConfigError`; anything else is a `ContractError` naming the
/// extern, the same "declaration is a hypothesis the target build already
/// enforces, so an unmapped failure names its origin" idiom `impl_op` uses
/// for a bespoke operation.
pub(super) fn error_match(ns: &str, func: &str, errors: &[ErrorBinding]) -> String {
    let contract_name = format!("{ns}.{func}");
    let fallback = format!(
        "TonoError::Contract(ContractError {{ contract_name: {contract_name:?}.to_string(), cause: e.to_string().into() }})"
    );
    // No declared sentinel leaves a single wildcard arm, which clippy flags
    // as a match on nothing (`match_single_binding`): spell the fallback
    // directly rather than routing it through a match with one arm.
    if errors.is_empty() {
        return fallback;
    }
    let mut arms = String::new();
    for e in errors {
        let _ = write!(
            arms,
            "{:?} => TonoError::Config(ConfigError {{ message: e.to_string() }}), ",
            e.sentinel,
        );
    }
    format!("match e.to_string().as_str() {{ {arms}_ => {fallback}, }}")
}

/// `yields`' non-error positions bound out of `Ok(..)`: a single position
/// binds bare, more than one destructures a tuple (the native `Result` only
/// carries one success value, so multiple `yields` names are always a tuple
/// of it).
fn ok_pattern(positions: &[&YieldsPos]) -> String {
    match positions {
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

/// The `dest = ...` (or field-by-field) assignment from the `Ok(..)` binding
/// into `dest`: a declared `returns:` projects fields one at a time, its
/// absence assigns the whole (possibly cast) binding.
fn ok_assign(
    dest: &str,
    ok_binding: &str,
    returns: Option<&ReturnsLit>,
    store: &dyn Fn(&str) -> String,
) -> String {
    let Some(returns) = returns else {
        return format!("{dest} = {};", store(ok_binding));
    };
    let fields: Vec<String> = returns
        .fields
        .iter()
        .map(|ReturnsField { name, value }| format!("{name}: {}", returns_value_expr(value)))
        .collect();
    let ty = type_ident_from_id(&match &returns.r#type {
        Tref::Ref { id, .. } => id.clone(),
        // A returns: type is always a declared named shape (the checker
        // enforces this); a primitive/list/map target has nothing to
        // project fields into and never reaches this branch.
        _ => String::new(),
    });
    format!(
        "{dest} = {};",
        store(&format!("{ty} {{ {} }}", fields.join(", ")))
    )
}

/// The extern-call assignment: resolves `ns.fn` against the
/// module's own `ext_libs`, emits the awaited call in the declared argument
/// order, destructures `yields`, projects `returns:`, and maps `errors:` —
/// or, absent `yields`, treats the call as a plain `Result<T, E>` whose `Ok`
/// assigns straight into `dest`.
pub(super) fn call_assign(
    r: &mut Resolver<'_, '_>,
    field: &EntryField,
    call: &EntryCall,
    dest: &str,
) -> String {
    let lib = find_lib(r.module, &call.ns);
    let decl = find_extern(lib, &call.func);
    let lang = find_rust_lang(decl);
    let errors = lang.errors.clone();
    let returns = lang.returns.clone();
    let ns = call.ns.clone();
    let func = call.func.clone();
    let yields = lang.yields.clone();
    let expr = call_expr(r, call);

    // The field's own slot may be stored wrapped (a foreign handle has no
    // zero value in Rust, so its draft starts unset); the call's result goes
    // in through the same wrapping every other source's does.
    let target = field.target.clone();
    let module = r.module;
    let store = move |expr: &str| super::ext::wrap_stored(&target, module, expr);

    let ok_positions: Vec<&YieldsPos> = yields.iter().filter(|y| !y.is_error).collect();

    let ok_pat = if yields.is_empty() {
        "v".to_string()
    } else {
        ok_pattern(&ok_positions)
    };
    format!(
        "match {expr} {{\n    Ok({ok_pat}) => {{ {assign} }}\n    Err(e) => {{ return Err({mapped}); }}\n}}",
        assign = ok_assign(dest, &ok_pat, returns.as_ref(), &store),
        mapped = error_match(&ns, &func, &errors),
    )
}

#[cfg(test)]
#[path = "resolve_call_tests.rs"]
mod tests;

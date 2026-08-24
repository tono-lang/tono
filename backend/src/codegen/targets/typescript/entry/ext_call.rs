//! The `= ns.fn(args)` extern-call field source: importing the foreign
//! symbol, calling it with the `ts` language block's own argument order
//! (including a `Ctor` struct literal, foreign field names verbatim),
//! projecting `yields`/`returns` onto the declared logical type, and mapping
//! a declared sentinel (or any unmapped failure) onto a typed error at the
//! `ContractError` boundary already used elsewhere in this target.
//!
//! TypeScript's own identity is "throws, and may return a Promise": nothing
//! in the IR marks a given extern call sync or async, and the compiler
//! cannot know statically whether a third-party function returns a Promise.
//! So every call is awaited unconditionally here (`await` on a plain value
//! is a safe no-op); `class_decl` (in `entry/mod.rs`) is what turns an entry
//! with at least one such field into an async-constructed client (a
//! `static async create`, not a plain `constructor`).
//!
//! Scope: a free function in an `ext` block (`module.ext_libs[].externs`),
//! including one whose logical return type is an opaque handle (the
//! `companybus.connect(..)` shape: no `yields`, the raw call result already
//! is the logical value). `entries::validate_entries` guarantees, before
//! this runs, that a call names a declared extern carrying a `ts` binding,
//! that a projecting `yields` names a `returns`, and that a `ts` binding
//! names at most one non-error `yields` position (a single call result has
//! nothing to bind a second one to); the lookups below still fail loudly on
//! a broken invariant rather than silently miscompiling. A bare
//! foreign-symbol call nested inside a `call:` line's own argument list
//! (`CallArg::SymbolCall`, e.g. `WithPrecision(precision)`) is supported.
//! Not yet supported: `CallArg::Call`, a *declared* cross-extern call
//! (`ns.fn(...)`) used as another call's argument -- reachable today only
//! from a ctor field's own value, not from `call:`'s own top level.
//! `TargetKind::emits_nested_extern_call_args` rejects this shape at
//! generation time before any emitter reaches it, so the panic below is
//! genuinely unreachable in a successful `tono gen` run; it stays as a
//! loud failure on a validation-gate bug rather than silently wrong output.
//! An opaque handle's own methods (`type publisher { extern send(..) }`,
//! invoked from an op's `impl`) are a different call site: `ext_handle_call`.
//!
//! ## The declared-test stub seam
//!
//! A generated hermetic test stubbing a free extern-fn call (`stub
//! companyconfig.load: ...`) needs to answer with the canned value in place
//! of the real call, without ever letting the real third-party package
//! resolve at Node's module-load time (a static top-level `import` is
//! resolved eagerly, unconditionally, whether or not the call the import
//! backs ever actually runs). The whole `try { await real(...); project;
//! } catch { map }` body therefore lives once, module-scoped, behind a
//! mutable `let` per extern-call field ([`seam_decls`]) rather than inline at
//! the field's own resolution point: an ESM import is a read-only binding,
//! so swapping *that* directly is not an option (the same reasoning
//! `impl_op::impl_seam_var` already applies to a bespoke `ext impl`). The
//! seam function receives the in-progress `Settings` draft as its own
//! parameter (matching the `s.<field>` spelling every argument already reads
//! off of), so a canned stub -- a plain `async () => value` closing over
//! nothing -- is freely assignable to it (a function declaring fewer
//! parameters than its target type is always assignable in TypeScript). The
//! stub's own answer is validated against the extern's declared *logical*
//! return type at the frontend, which is exactly what
//! this seam's own return type is: swapping it in skips the real call, the
//! `yields`/`returns` projection, and the error mapping in one step, rather
//! than trying to fake a raw foreign result the projection would have to
//! re-derive from.

use std::collections::BTreeSet;

use super::{
    field_camel, field_ts_type, foreign_handle, module_symbol, pascal, Helpers, Names, Resolver,
};
use crate::codegen::entries::EntryModel;
use crate::codegen::foreign_spelling;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{
    ArmValue, CallArg, EntryCall, EntryField, ExtLib, ExternLang, ExternParam, Module,
    ReturnsValue, Select,
};

/// The module-local binding an extern-call field's generated leaf invokes
/// instead of the imported foreign symbol directly: see the module-level doc
/// for why. `swap_fn` names the exported test-only swapper it works with.
pub(super) fn ext_seam_var(n: &Names, field: &EntryField) -> String {
    super::camel(&format!("{}{}_ext", n.op_prefix, field.name))
}

/// The exported swapper a generated test simulates a free extern-fn stub
/// through.
pub(super) fn ext_swap_fn_name(n: &Names, field: &EntryField) -> String {
    format!(
        "swap{}ExtForTest",
        super::pascal(&format!("{}{}", n.op_prefix, field.name))
    )
}

pub(super) use super::ext_args::{field_ref, json_literal, render_arg};

/// A spelling as the emitted TypeScript writes it: verbatim, with a
/// reference to one of the module's own types rendered as the type it
/// generates (`Memo<.reading>` is `Memo<Reading>`). The library's names
/// are not qualified here: `import_spelling` brings them into scope.
pub(super) fn spell(spelling: &str, module: &Module) -> String {
    foreign_spelling::render(spelling, &crate::codegen::entries::generated_type(module))
}

/// The library identifiers a spelling names, to import from the lib's own
/// TypeScript module: `new ConstantCalculator` imports `ConstantCalculator`,
/// `FormulaCalculator.parse` imports `FormulaCalculator`,
/// `Calculator<number>[]` imports `Calculator`. Builtins need no import,
/// and a reference to one of the module's own types is not the library's
/// to import; every other word is, whatever the module generates under the
/// same name.
pub(super) fn import_spelling(spelling: &str, lib: &ExtLib, refs: &mut Vec<Symbol>) {
    let names = foreign_spelling::library_names(spelling, &foreign_spelling::ts_builtin);
    if names.is_empty() {
        return;
    }
    let path = lib
        .langs
        .iter()
        .find(|p| p.lang == "ts" || p.lang == "typescript")
        .unwrap_or_else(|| {
            panic!(
                "ext lib {:?} declares no typescript module path (validate_entries should have rejected this)",
                lib.name
            )
        });
    for name in names {
        refs.push(Symbol::imported(name.clone(), path.path.clone(), name));
    }
}

/// The TypeScript spelling of a declared handle's class, for a class
/// reference: the head of the storage type its `ts` block declares
/// (`AnswerCalculator`), the one thing that can stand as a value. A handle
/// with no `ts` block has no class to pass;
/// `validate_calls::handle_storage_declared` refuses it first.
pub(super) fn class_reference_name(lib: &ExtLib, handle: &str) -> String {
    let ty = lib.types.iter().find(|t| t.name == handle).unwrap_or_else(|| {
        panic!(
            "a class reference names undeclared handle {handle:?} in ext lib {:?} (the frontend should have rejected this)",
            lib.name
        )
    });
    let storage = ty.storage("ts").unwrap_or_else(|| {
        panic!(
            "handle {handle:?} declares no ts block; validate_calls::handle_storage_declared should have refused it"
        )
    });
    foreign_spelling::library_names(storage, &foreign_spelling::ts_builtin)
        .into_iter()
        .next()
        .unwrap_or_else(|| storage.to_string())
}

/// One import per class reference anywhere in a `call:` line's argument
/// tree, from the lib's own TypeScript module path (the same module the
/// call's symbol comes from).
pub(super) fn class_reference_imports(args: &[CallArg], lib: &ExtLib, refs: &mut Vec<Symbol>) {
    for arg in args {
        match arg {
            CallArg::TypeRef(handle) => {
                let name = class_reference_name(lib, handle);
                import_spelling(&name, lib, refs);
            }
            // A nested foreign call names a symbol of the same module.
            CallArg::SymbolCall(sc) => {
                import_spelling(&sc.symbol, lib, refs);
                class_reference_imports(&sc.args, lib, refs);
            }
            CallArg::List(items) => class_reference_imports(items, lib, refs),
            CallArg::Ctor(ctor) => {
                for v in ctor.fields.values() {
                    class_reference_imports(std::slice::from_ref(v), lib, refs);
                }
            }
            CallArg::Param(_)
            | CallArg::ParamAs { .. }
            | CallArg::Foreign(_)
            | CallArg::Ref(_)
            | CallArg::Lit(_)
            | CallArg::Call(_) => {}
        }
    }
}

/// A path rooted at the single bound `yields` name, read off the awaited
/// call's raw result: the head (the `yields:` position name itself) becomes
/// `raw`, the rest stays verbatim (a foreign struct's own field names, never
/// cased).
pub(super) fn foreign_path_expr(yields_name: &str, path: &[String]) -> String {
    let (head, rest) = path
        .split_first()
        .unwrap_or_else(|| panic!("a returns: value has no path segments"));
    assert_eq!(
        head, yields_name,
        "a returns: value references a yields name other than the one bound here, which is not supported yet"
    );
    if rest.is_empty() {
        "raw".to_string()
    } else {
        format!("raw.{}", rest.join("."))
    }
}

pub(super) fn arm_value_expr(yields_name: &str, value: &ArmValue) -> String {
    match value {
        ArmValue::Field(path) => foreign_path_expr(yields_name, path),
        ArmValue::Lit(v) => json_literal(v),
        // A declared-source chain names an entry field's own resolution, not
        // a foreign value the extern call just produced; a `returns:` match
        // arm inside an extern binding has nothing to run that chain over.
        ArmValue::Sources(_) => {
            unreachable!("a returns: match arm cannot bind a declared-source chain")
        }
        ArmValue::Subject => {
            unreachable!("a returns: match arm cannot bind the entry-field match subject")
        }
    }
}

/// A `match` inside `returns:`, lowered to an immediately invoked switch:
/// TypeScript has no match expression, so the arms return from a wrapper
/// function instead of assigning through a shared destination the way a
/// statement-level `switch` (`plan::Stmt::Switch`) does elsewhere in this
/// target. The subject and every arm read off the same awaited raw result
/// `returns:`'s bare-field case does.
pub(super) fn select_expr(yields_name: &str, select: &Select) -> String {
    let subject = foreign_path_expr(yields_name, &select.subject);
    let mut arms = String::new();
    for arm in &select.arms {
        let value = arm_value_expr(yields_name, &arm.value);
        match &arm.pattern {
            Some(pattern) => arms.push_str(&format!(
                "    case {}: return {value};\n",
                json_literal(pattern)
            )),
            None => arms.push_str(&format!("    default: return {value};\n")),
        }
    }
    format!("(() => {{\n  switch ({subject}) {{\n{arms}  }}\n}})()")
}

/// A `returns:` field's value, projected off the single bound `yields` name.
pub(super) fn returns_value_expr(yields_name: &str, value: &ReturnsValue) -> String {
    match value {
        ReturnsValue::Field(path) => foreign_path_expr(yields_name, path),
        ReturnsValue::Select(select) => select_expr(yields_name, select),
    }
}

/// The extern lookup [`call_body`] and [`seam_decls`] share: the declared
/// `ext` lib, its extern, and both their `ts`/`typescript` language blocks.
/// `entries::validate_entries` (`call_resolves`) has already confirmed all of
/// this for the typescript target before generation reaches here, so every
/// lookup is an internal-invariant check, not a validation.
struct Lookup<'a> {
    lib: &'a ExtLib,
    decl: &'a crate::ir::ExternDecl,
    lang: &'a ExternLang,
    params: &'a [ExternParam],
}

fn lookup<'a>(module: &'a Module, field: &EntryField, call: &EntryCall) -> Lookup<'a> {
    let lib = module
        .ext_libs
        .iter()
        .find(|l| l.name == call.ns)
        .unwrap_or_else(|| {
            panic!(
                "entry field {:?} calls undeclared ext lib {:?} (validate_entries should have rejected this)",
                field.name, call.ns
            )
        });
    let decl = lib
        .externs
        .iter()
        .find(|e| e.name == call.func)
        .unwrap_or_else(|| {
            panic!(
                "entry field {:?} calls undeclared extern {:?} in ext lib {:?} (validate_entries should have rejected this)",
                field.name, call.func, lib.name
            )
        });
    let lang = decl
        .langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
        .unwrap_or_else(|| {
            panic!(
                "extern {}.{} has no typescript binding (validate_entries should have rejected this)",
                call.ns, call.func
            )
        });
    Lookup {
        lib,
        lang,
        decl,
        params: &decl.params,
    }
}

/// The try/catch body a seam function performs: the real call, its
/// `yields`/`returns` projection (or bare pass-through), and its error
/// boundary, ending in `return` rather than an assignment (this is always
/// the seam's own body now -- see the module doc for why the assignment
/// itself lives at the field's own resolution point instead).
#[allow(clippy::too_many_arguments)]
fn call_body(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    field: &EntryField,
    call: &EntryCall,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
) -> String {
    let l = lookup(module, field, call);
    let lang = l.lang;

    refs.push(module_symbol(&error_names().contract, module));
    class_reference_imports(&lang.call_args, l.lib, refs);

    let args = {
        let mut parts = Vec::with_capacity(lang.call_args.len());
        for a in &lang.call_args {
            parts.push(render_arg(
                entry, config, module, l.lib, a, l.params, &call.args, &field_ref,
            ));
        }
        parts.join(", ")
    };
    let call_name = format!("{}.{}", call.ns, call.func);

    // A `ts` binding never reads its own `error` yields position (the
    // catch below is the only error channel a thrown Promise rejection
    // gives this target); `validate_entries` guarantees at most one
    // non-error position, which is the only one worth projecting.
    let assign = match (
        lang.yields.iter().find(|y| !y.is_error),
        lang.returns.as_ref(),
    ) {
        // No projection: the extern's own construction result already is
        // the logical value (see the module doc), whether the binding left
        // the positions to the convention or named the one it returns. For
        // a foreign-handle field that "already is" now has a real generated
        // interface to be, so the seam narrows the honestly-`unknown` raw
        // result into it here, once, at the exact tono-declared
        // construction boundary -- not a reusable projection, just this one
        // field trusting the frontend's own guarantee that this call's
        // declared logical type is the field's own declared type.
        (_, None) if foreign_handle(&field.target, module) => {
            format!("return raw as {};", field_ts_type(&field.target, module))
        }
        (_, None) => "return raw;".to_string(),
        (None, Some(_)) => panic!(
            "extern {call_name} declares a returns but no yields position to project from (validate_entries should have rejected this)"
        ),
        (Some(y), Some(returns)) => {
            let projected = returns
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        field_camel(&f.name, config),
                        returns_value_expr(&y.name, &f.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("return {{ {projected} }};")
        }
    };

    let switch = sentinel_switch(
        &l.decl.errors,
        l.lib,
        module,
        refs,
        sentinel_types,
        &|expr| format!("throw {expr};"),
    );

    // The callee spelling is written as is, its library identifiers
    // imported: `new ConstantCalculator(value)` constructs a class
    // (synchronous by construction), `FormulaCalculator.parse(expr)` calls
    // a static method, and a call the op declares asynchronous for this
    // target is awaited. The surrounding seam stays `async` regardless (an
    // entry with any extern-call field is already async-constructed; this
    // one call's own expression is the only thing that changes).
    import_spelling(&lang.symbol, l.lib, refs);
    let symbol = spell(&lang.symbol, module);
    let call_expr = match foreign_spelling::constructed(&symbol) {
        Some(class) => format!("new {class}({args})"),
        None if l.decl.is_async("ts") => format!("await {symbol}({args})"),
        None => format!("{symbol}({args})"),
    };

    format!(
        "try {{\n  const raw = {call_expr};\n  {assign}\n}} catch (e) {{\n{switch}  throw new {contract}({call_name:?}, e);\n}}",
        contract = error_names().contract,
    )
}

/// The `catch (e) { ... }` checks mapping every error the op declares
/// (`@errors`, in declared order) to its typed error class, recognized the
/// way its own `ts` block says: the block names the class the library
/// throws (`e instanceof ParseError`), imported from the library's module.
/// Shared by a free extern-fn call's own seam ([`call_body`]) and an op's
/// own handle-method call (`ext_handle_call::impl_call_body`); `throw`
/// composes the caller's own exit statement. Empty when no declared error
/// has a `ts` block, so a call with nothing to recognize adds no dead check.
pub(super) fn sentinel_switch(
    errors: &[String],
    lib: &ExtLib,
    module: &Module,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
    throw: &dyn Fn(String) -> String,
) -> String {
    let mut checks = String::new();
    for id in errors {
        let Some(fl) = crate::ir::ForeignLang::of_error(module, id, "ts") else {
            continue;
        };
        let type_name = crate::codegen::entries::local_name(id);
        let class_name = sentinel_error_class(type_name);
        sentinel_types.insert(type_name.to_string());
        refs.push(module_symbol(&class_name, module));
        import_spelling(&fl.name, lib, refs);
        checks.push_str(&format!(
            "  if (e instanceof {sentinel}) {{ {t} }}\n",
            sentinel = spell(&fl.name, module),
            t = throw(format!("new {class_name}(e)")),
        ));
    }
    checks
}

/// The per-field seam bindings of an entry's extern-call fields: one mutable
/// `let` per field sourced by a free-fn `ext` call or by a handle-method
/// call (an async function taking the in-progress `Settings` draft and
/// performing the real call/projection/error-mapping the field's own
/// resolution step used to inline directly), plus (only when the entry
/// declares tests) the exported swapper a generated test goes through to
/// stub that call. An op's own `impl` body reaching a handle's method is a
/// different call site with a seam of its own
/// (`ext_handle_call::op_seam_decls`), not seamed here.
pub(super) fn seam_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut Helpers,
    has_tests: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    for field in entry.fields.iter().copied() {
        if let Some(call) = &field.handle_call {
            decls.push(handle_call_seam_decl(
                entry, n, module, config, helpers, has_tests, field, call,
            ));
            continue;
        }
        let Some(call) = &field.call else { continue };
        let l = lookup(module, field, call);
        let seam = ext_seam_var(n, field);
        let mut refs = Vec::new();
        let mut sentinel_types = BTreeSet::new();
        let body = call_body(
            entry,
            config,
            module,
            field,
            call,
            &mut refs,
            &mut sentinel_types,
        );
        helpers.ext_error_types.extend(sentinel_types);
        let target_ty = field_ts_type(&field.target, module);
        refs.extend(super::type_refs(&field.target, module));

        let mut text = format!(
            "// {seam} is the call the generated field resolver goes through to reach\n\
             // the foreign {sym} (an ESM import is a read-only binding, so a test that\n\
             // simulates an outcome swaps this module-local one instead).\n\
             let {seam}: (s: {settings}) => Promise<{target_ty}> = async (s) => {{\n{body}\n}};",
            sym = foreign_spelling::constructed(&l.lang.symbol).unwrap_or(&l.lang.symbol),
            settings = n.settings,
            body = indent_body(&body),
        );
        if has_tests {
            text.push_str(&swap_fn_text(n, field, &seam));
        }
        decls.push(Decl::raw_with(text, refs));
    }
    decls
}

/// One field's seam for a `= .field.method(args)` source: the same
/// module-scoped `let` a free call's seam takes (so the field's own
/// resolution step is the one-line `dest = await seam(s)` either way), with
/// no foreign import of its own, since the call goes through the receiver
/// field the draft already holds.
#[allow(clippy::too_many_arguments)]
fn handle_call_seam_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut Helpers,
    has_tests: bool,
    field: &EntryField,
    call: &crate::ir::OpImplCall,
) -> Decl {
    let seam = ext_seam_var(n, field);
    let mut refs = Vec::new();
    let mut sentinel_types = BTreeSet::new();
    let body = super::ext_handle_call::handle_call_body(
        entry,
        config,
        module,
        field,
        call,
        &mut refs,
        &mut sentinel_types,
    );
    helpers.ext_error_types.extend(sentinel_types);
    let target_ty = field_ts_type(&field.target, module);
    refs.extend(super::type_refs(&field.target, module));
    let call_name = format!(".{}.{}", call.recv.join("."), call.method);
    let mut text = format!(
        "// {seam} is the call the generated field resolver goes through to reach\n\
         // {call_name} on the resolved handle (a test that simulates an outcome swaps\n\
         // this module-local one instead).\n\
         let {seam}: (s: {settings}) => Promise<{target_ty}> = async (s) => {{\n{body}\n}};",
        settings = n.settings,
        body = indent_body(&body),
    );
    if has_tests {
        text.push_str(&swap_fn_text(n, field, &seam));
    }
    Decl::raw_with(text, refs)
}

/// The exported swapper a generated test simulates a stub through, for one
/// field's seam.
fn swap_fn_text(n: &Names, field: &EntryField, seam: &str) -> String {
    let swap = ext_swap_fn_name(n, field);
    format!(
        "\n\n// {swap} swaps the extern call {field} goes through and returns the\n\
         // previous one; a test generated from the declared tests simulates an\n\
         // outcome with it and restores the real call afterwards.\n\
         export function {swap}(next: typeof {seam}): typeof {seam} {{\n\
         \x20 const prev = {seam};\n\
         \x20 {seam} = next;\n\
         \x20 return prev;\n\
         }}",
        field = field.name,
    )
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

/// The typed-error class name a declared sentinel maps to: the bare
/// identifier the `.tono` author wrote (`overloaded`), cased through the
/// same engine as every other generated identifier, suffixed the way every
/// other category in the closed taxonomy is (`ContractError`,
/// `ConfigError`, ...).
pub(super) fn sentinel_error_class(sentinel_type: &str) -> String {
    format!("{}Error", pascal(sentinel_type))
}

/// The generated class for one distinct sentinel-mapped type name, emitted
/// once per module regardless of how many extern calls declare it. Rooted
/// under the same taxonomy root as every other category, so the existing
/// `instanceof TonoError` boundary checks still see it.
pub(super) fn sentinel_error_decl(sentinel_type: &str, module: &Module) -> Decl {
    let name = sentinel_error_class(sentinel_type);
    let root = error_names().root;
    Decl::raw_with(
        format!(
            "// {name} is the typed error a declared ext sentinel maps to; a\n\
         // third-party call that throws this sentinel surfaces as this class\n\
         // instead of the generic ContractError fallback.\n\
         export class {name} extends {root} {{\n\
         \x20 constructor(readonly cause: unknown) {{\n\
         \x20   super({sentinel_type:?});\n\
         \x20   this.name = {name:?};\n\
         \x20 }}\n\
         }}",
        ),
        vec![module_symbol(&root, module)],
    )
}

/// [`plan::Emitter::call_assign`] for TypeScript: hands the field's own value
/// off to the module-scoped seam ([`seam_decls`]), passing it the in-progress
/// `Settings` draft any of the seam's own argument rendering reads sibling
/// fields off of. `dest` is already a ready-to-use assignment target (a
/// top-level field or a config member path), supplied by the shared plan.
pub(super) fn call_assign(
    _r: &mut Resolver,
    field: &EntryField,
    _call: &EntryCall,
    dest: &str,
    n: &Names,
) -> String {
    let seam = ext_seam_var(n, field);
    format!("{dest} = await {seam}(s);")
}

/// [`plan::Emitter::handle_call_assign`] for TypeScript: the same hand-off
/// to the field's own module-scoped seam a free call takes; the seam body
/// is the handle-method call (`ext_handle_call::handle_call_body`).
pub(super) fn handle_call_assign(
    _r: &mut Resolver,
    field: &EntryField,
    _call: &crate::ir::OpImplCall,
    dest: &str,
    n: &Names,
) -> String {
    let seam = ext_seam_var(n, field);
    format!("{dest} = await {seam}(s);")
}

#[cfg(test)]
#[path = "ext_call_tests.rs"]
mod tests;

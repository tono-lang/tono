//! Rendering a call's own arguments, its `returns:` projection, and its
//! `errors:` mapping as Go expressions/statements: the pure, recursive part
//! of emitting one extern call, shared by a field's own construction call
//! and an op's `impl .field.method(args)` body (`build_call`/`call_assign`/
//! `impl_call_body`, still in `ext`). Split out of `ext` to keep it under
//! the file-size ceiling; `use super::*` reaches the parent's lookup
//! helpers (`lib_go_path`, `lib_ident`, `import_lib`, `foreign_handle`,
//! `handle_go_type`).

use super::*;

fn literal_of_json(v: &serde_json::Value) -> String {
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
/// The Go element type of a variadic parameter's collection, or `None` when
/// it cannot be resolved (a non-handle logical type -- the bench only
/// exercises a handle-typed collection, `Vec<Box<dyn Calculator<T>>>`'s Go
/// counterpart -- or a handle the `ext` block does not declare). Reused for
/// both a variadic slice's own element type and its spread.
fn variadic_element_go_type(
    module: &Module,
    param: &ExternParam,
    refs: &mut Vec<Symbol>,
) -> Option<String> {
    let (handle_lib, handle_ty) = foreign_handle(&param.r#type, module)?;
    let handle = handle_lib.types.iter().find(|t| t.name == handle_ty)?;
    handle_go_type(handle_lib, handle, refs)
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn call_arg_expr(
    refs: &mut Vec<Symbol>,
    module: &Module,
    lib: &ExtLib,
    arg: &CallArg,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    match arg {
        CallArg::Param(name) => {
            let idx = params.iter().position(|p| p.name == *name);
            let param = idx.map(|i| &params[i]);
            match idx.and_then(|i| entry_args.get(i)) {
                // A variadic parameter's own actual argument is, by
                // construction, a `CallArg::List`: spread it as a typed
                // slice (`[]GoType{...}...`) instead of recursing into the
                // generic, untyped `[]any{...}` the `List` branch below
                // renders -- Go cannot spread `[]any` into `...Option`.
                Some(CallArg::List(items)) if param.is_some_and(|p| p.variadic) => {
                    let elem_ty = variadic_element_go_type(module, param.unwrap(), refs)
                        .unwrap_or_else(|| {
                            panic!(
                                "cannot resolve the Go element type of variadic parameter {:?}",
                                param.unwrap().name
                            )
                        });
                    let rendered: Vec<String> = items
                        .iter()
                        .map(|a| call_arg_expr(refs, module, lib, a, params, entry_args, ref_expr))
                        .collect();
                    format!("[]{elem_ty}{{{}}}...", rendered.join(", "))
                }
                Some(actual) => {
                    call_arg_expr(refs, module, lib, actual, params, entry_args, ref_expr)
                }
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
                .map(|a| call_arg_expr(refs, module, lib, a, params, entry_args, ref_expr))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Ctor(ctor) => ctor_expr(refs, module, lib, ctor, params, entry_args, ref_expr),
        // A cross-extern call standing as another call's own argument (e.g.
        // a ctor field's value naming a declared extern): Go codegen has no
        // case for it yet. `TargetKind::emits_nested_extern_call_args`
        // rejects this shape at generation time before any emitter reaches
        // it, so this is unreachable in a successful `tono gen` run --
        // panicking on it (rather than emitting `nil` and letting `go
        // build` fail somewhere else) surfaces a validation-gate bug
        // loudly instead of miscompiling silently.
        CallArg::Call(_) => panic!(
            "a cross-extern call as a call argument reached Go codegen; \
             validate_calls::extern_binds_every_target should have rejected it first"
        ),
        // Go has no type as a value to pass; `validate_calls` refuses the
        // binding by name before generation reaches here.
        CallArg::TypeRef(_) => panic!(
            "a class reference as a call argument reached Go codegen; \
             validate_calls::class_reference_renders should have rejected it first"
        ),
        // A bare foreign-symbol call nested inside a `call:` line's own
        // argument list, e.g. `WithPrecision(precision)`: no declared
        // extern to resolve against, so no yields/returns/errors
        // projection -- rendered as a plain synchronous, infallible call,
        // the shape the bench's own nested calls have. The symbol lives in
        // the same package as the enclosing `call:`, so it is qualified and
        // imported the same way `ctor_expr` qualifies a foreign struct.
        CallArg::SymbolCall(sc) => {
            let Some(path) = lib_go_path(lib) else {
                return "nil".to_string();
            };
            let ident = lib_ident(&lib.name);
            refs.push(import(&ident, path));
            format!(
                "{ident}.{}({})",
                sc.symbol,
                sc.args
                    .iter()
                    .map(|a| call_arg_expr(refs, module, lib, a, params, entry_args, ref_expr))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn ctor_expr(
    refs: &mut Vec<Symbol>,
    module: &Module,
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
            let expr = call_arg_expr(refs, module, lib, value, params, entry_args, ref_expr);
            // Mirrors the foreign side's own field spelling verbatim, like
            // a declared foreign struct's fields: the ctor's field key is
            // written exactly as the caller declared it.
            format!("{name}: {expr}")
        })
        .collect();
    format!("{ident}.{}{{{}}}", pascal(&ctor.name), fields.join(", "))
}

fn yields_path_expr(yields_vars: &HashMap<String, String>, path: &[String]) -> String {
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

fn member_type(module: &Module, target: &Tref, member: &str) -> Option<Tref> {
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
pub(in super::super) fn returns_expr(
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
                        // Same reasoning as `Sources` above: a `returns:`
                        // match arm has no entry-field subject to narrow
                        // here, only the foreign yields binding.
                        ArmValue::Subject => "nil".to_string(),
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
pub(in super::super) fn declared_error_literal(
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
        .map(|m| crate::codegen::conventions::field_ident(m, config, super::super::LANG));
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
pub(in super::super) fn error_block(
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
pub(in super::super) fn has_error_position(lang: &ExternLang) -> bool {
    lang.yields.iter().any(|y| y.is_error)
}

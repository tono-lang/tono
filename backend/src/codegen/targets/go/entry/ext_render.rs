//! Rendering a call's own arguments, its `returns:` projection, and its
//! `errors:` mapping as Go expressions/statements: the pure, recursive part
//! of emitting one extern call, shared by a field's own construction call
//! and an op's `impl .field.method(args)` body (`build_call`/`call_assign`/
//! `impl_call_body`, still in `ext`). Split out of `ext` to keep it under
//! the file-size ceiling; `use super::*` reaches the parent's lookup
//! helpers (`lib_go_path`, `lib_ident`, `import_lib`, `foreign_handle`,
//! `handle_go_type`).

use super::*;

pub(in super::super) fn literal_of_json(v: &serde_json::Value) -> String {
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
/// The conversion a parameter spelled under its own Go type goes through:
/// a variadic slot (`...T`) spreads the caller's collection, a builtin
/// numeric type converts (`int(v)`), the parameter's own default spelling
/// passes as is. Anything else has no conversion Go can write, and
/// `validate_calls::param_spelling_coerces` refuses it before generation.
pub(in super::super) fn coerce(
    module: &Module,
    lib: &ExtLib,
    param_type: &Tref,
    spelling: &str,
    expr: &str,
    list_items: Option<&[String]>,
) -> Result<String, String> {
    let alias = lib_ident(&lib.name);
    if let Some(elem) = foreign_spelling::variadic(spelling) {
        let elem_ty = qualify(elem, &alias, module);
        return Ok(match list_items {
            Some(items) => format!("[]{elem_ty}{{{}}}...", items.join(", ")),
            None => format!("{expr}..."),
        });
    }
    let default = spelled_type(module, param_type);
    if go_builtin(spelling) && spelling != default {
        return Ok(format!("{spelling}({expr})"));
    }
    if qualify(spelling, &alias, module) == default {
        return Ok(expr.to_string());
    }
    Err(format!(
        "cannot pass a {default} as {spelling} in Go: no conversion from {default} to {spelling}"
    ))
}

/// The conversion a struct literal spelled under its own Go type goes
/// through: `&T` takes the address of the literal (`&mathkit.Options{..}`,
/// for a library that takes the form by pointer), the form's own type
/// passes it as is. The form's type is what its `go` block declares; the
/// `&` belongs to the argument, never to that declaration (the check
/// probes the form as a value, `func(tonoForm mathkit.Options)`). Anything
/// else has no conversion Go can write, and
/// `validate_calls::foreign_forms_declared` refuses it before generation.
pub(in super::super) fn form_coerce(
    module: &Module,
    lib: &ExtLib,
    block: &ForeignLang,
    spelling: &str,
    literal: &str,
) -> Result<String, String> {
    let alias = lib_ident(&lib.name);
    let form_type = qualify(block.head(), &alias, module);
    if qualify(spelling, &alias, module) == form_type {
        return Ok(literal.to_string());
    }
    if let Some(inner) = spelling.strip_prefix('&') {
        if qualify(inner.trim_start(), &alias, module) == form_type {
            return Ok(format!("&{literal}"));
        }
    }
    Err(format!(
        "cannot pass a {form_type} literal as {spelling} in Go: no conversion from {form_type} to {spelling}"
    ))
}

/// The Go type a logical type already has at the boundary: a handle's
/// declared storage (or a slice of them), else the ordinary mapping.
fn spelled_type(module: &Module, t: &Tref) -> String {
    match t {
        Tref::List(inner) => format!("[]{}", spelled_type(module, inner)),
        _ => foreign_handle(t, module)
            .and_then(|(lib, ty)| {
                let handle = lib.types.iter().find(|h| h.name == ty)?;
                handle_go_type(lib, handle, module, &mut Vec::new())
            })
            .unwrap_or_else(|| go_type(t)),
    }
}

#[allow(clippy::too_many_arguments)]
pub(in super::super) fn call_arg_expr(
    refs: &mut Vec<Symbol>,
    module: &Module,
    lib: &ExtLib,
    arg: &CallArg,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ctx_expr: &str,
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    match arg {
        CallArg::Param(name) => {
            let idx = params.iter().position(|p| p.name == *name);
            match idx.and_then(|i| entry_args.get(i)) {
                Some(actual) => call_arg_expr(
                    refs, module, lib, actual, params, entry_args, ctx_expr, ref_expr,
                ),
                // Unreachable through `tono check`: an extern's own params
                // and its per-language `call:` args are arity-checked
                // against each other on the frontend.
                None => "nil".to_string(),
            }
        }
        // The parameter crosses under its own Go spelling: the caller's
        // actual argument, converted as the spelling asks (a variadic slot
        // spreads a list literal as a typed slice, `[]T{...}...`, since Go
        // cannot spread `[]any`).
        CallArg::ParamAs { name, spelling } => {
            let idx = params.iter().position(|p| p.name == *name);
            let (Some(param), Some(actual)) =
                (idx.map(|i| &params[i]), idx.and_then(|i| entry_args.get(i)))
            else {
                return "nil".to_string();
            };
            let items: Option<Vec<String>> = match actual {
                CallArg::List(items) => Some(
                    items
                        .iter()
                        .map(|a| {
                            call_arg_expr(
                                refs, module, lib, a, params, entry_args, ctx_expr, ref_expr,
                            )
                        })
                        .collect(),
                ),
                _ => None,
            };
            let expr = match &items {
                Some(_) => String::new(),
                None => call_arg_expr(
                    refs, module, lib, actual, params, entry_args, ctx_expr, ref_expr,
                ),
            };
            coerce(
                module,
                lib,
                &param.r#type,
                spelling,
                &expr,
                items.as_deref(),
            )
            .unwrap_or_else(|e| {
                panic!("{e}; validate_calls::param_spelling_coerces should have refused it")
            })
        }
        // A declared position: the context, bound by the emitter (the
        // spelling is checked against `ctx context.Context` by
        // `validate_calls::foreign_position_binds`).
        CallArg::Foreign(_) => ctx_expr.to_string(),
        CallArg::Ref(path) => ref_expr(path),
        CallArg::Lit(v) => literal_of_json(v),
        CallArg::List(items) => format!(
            "[]any{{{}}}",
            items
                .iter()
                .map(|a| call_arg_expr(
                    refs, module, lib, a, params, entry_args, ctx_expr, ref_expr
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Ctor(ctor) => ctor_expr(
            refs, module, lib, ctor, params, entry_args, ctx_expr, ref_expr,
        ),
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
            let Some(alias) = import_lib(refs, lib) else {
                return "nil".to_string();
            };
            let rendered: Vec<String> = sc
                .args
                .iter()
                .map(|a| {
                    call_arg_expr(refs, module, lib, a, params, entry_args, ctx_expr, ref_expr)
                })
                .collect();
            format!(
                "{}({})",
                qualify(&sc.symbol, &alias, module),
                rendered.join(", ")
            )
        }
    }
}

/// A struct literal: a foreign form's own Go type, from its `go` block,
/// with each field's value converted when the block spells the field under
/// its own type, and the whole literal converted when the argument spells
/// how it crosses (`&Options`); or, when the name is none of the lib's
/// forms, the literal of one of the module's own structs (see
/// [`generated_ctor_expr`]). A form with no `go` block does not exist in
/// Go; `validate_calls::foreign_forms_declared` refuses the binding first.
#[allow(clippy::too_many_arguments)]
fn ctor_expr(
    refs: &mut Vec<Symbol>,
    module: &Module,
    lib: &ExtLib,
    ctor: &CallCtor,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ctx_expr: &str,
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    let Some(alias) = import_lib(refs, lib) else {
        return "nil".to_string();
    };
    let form = lib.structs.iter().find(|s| s.name == ctor.name);
    if form.is_none() {
        return generated_ctor_expr(
            refs, module, lib, ctor, params, entry_args, ctx_expr, ref_expr,
        );
    }
    let Some(block) = form.and_then(|f| f.lang("go")) else {
        panic!(
            "foreign struct {:?} declares no go block; validate_calls::foreign_forms_declared should have refused it",
            ctor.name
        );
    };
    let fields: Vec<String> = ctor
        .fields
        .iter()
        .map(|(name, value)| {
            let expr = call_arg_expr(
                refs, module, lib, value, params, entry_args, ctx_expr, ref_expr,
            );
            let declared = form.and_then(|f| f.fields.iter().find(|ff| ff.name == *name));
            let expr = match (block.fields.get(name), declared) {
                (Some(spelling), Some(ff)) => {
                    coerce(module, lib, &ff.r#type, spelling, &expr, None).unwrap_or_else(|e| {
                        panic!("{e}; validate_calls::field_spelling_coerces should have refused it")
                    })
                }
                _ => expr,
            };
            // Mirrors the foreign side's own field spelling verbatim, like
            // a declared foreign struct's fields: the ctor's field key is
            // written exactly as the caller declared it.
            format!("{name}: {expr}")
        })
        .collect();
    let literal = format!(
        "{}{{{}}}",
        qualify(block.head(), &alias, module),
        fields.join(", ")
    );
    match &ctor.spelling {
        None => literal,
        Some(spelling) => form_coerce(module, lib, block, spelling, &literal).unwrap_or_else(|e| {
            panic!("{e}; validate_calls::foreign_forms_declared should have refused it")
        }),
    }
}

/// The literal of one of the module's own structs, built where a call
/// passes it (`cfg { host: .h }` into a library generic over the caller's
/// type): the generated type's own composite literal, `Cfg{Host: h}`, the
/// same value the binding could pass by reference to a field of that type.
/// The type and its fields are the ones the types file emits (the casing
/// engine, `@rename(go)` honored), in the same package as this glue, so
/// nothing is qualified or imported; an optional member the literal leaves
/// out keeps Go's zero value. No foreign block is involved: the struct is
/// not the library's. `validate_calls::foreign_forms_declared` refuses a
/// name that is not a buildable wire struct before generation.
#[allow(clippy::too_many_arguments)]
fn generated_ctor_expr(
    refs: &mut Vec<Symbol>,
    module: &Module,
    lib: &ExtLib,
    ctor: &CallCtor,
    params: &[ExternParam],
    entry_args: &[CallArg],
    ctx_expr: &str,
    ref_expr: &mut dyn FnMut(&[String]) -> String,
) -> String {
    let (shape, members) = crate::codegen::entries::literal_struct(module, &ctor.name)
        .unwrap_or_else(|| {
            panic!(
                "struct literal {:?} names no form of ext {} and no wire struct of module {}; validate_calls::foreign_forms_declared should have refused it",
                ctor.name, lib.name, module.name
            )
        });
    let config = crate::codegen::targets::go::types::go_casing();
    let fields: Vec<String> = ctor
        .fields
        .iter()
        .map(|(name, value)| {
            let member = members
                .iter()
                .find(|m| m.name == *name)
                .unwrap_or_else(|| {
                    panic!("struct {} declares no field {name}; validate_calls::foreign_forms_declared should have refused it", ctor.name)
                });
            let expr = call_arg_expr(
                refs, module, lib, value, params, entry_args, ctx_expr, ref_expr,
            );
            format!(
                "{}: {expr}",
                crate::codegen::conventions::field_ident(member, &config, "go")
            )
        })
        .collect();
    format!(
        "{}{{{}}}",
        crate::codegen::conventions::type_ident(shape, "go"),
        fields.join(", ")
    )
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

/// The declared-error literal a recognized foreign error builds: the
/// shape's own Go type, each field the `go` block maps filled from its
/// source on the matched value (`Message: err.Error()`,
/// `RetryAfter: target.RetryAfter`); an unmapped field keeps its zero
/// value.
pub(in super::super) fn declared_error_literal(
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    shape_id: &str,
    source: &str,
    fields: &std::collections::BTreeMap<String, String>,
) -> String {
    let ty = pascal(crate::codegen::entries::local_name(shape_id));
    let members = module
        .shapes
        .iter()
        .find(|s| s.id == shape_id)
        .and_then(|shape| match &shape.kind {
            ShapeKind::Structure { members, .. } => Some(members),
            _ => None,
        });
    let parts: Vec<String> = members
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let spelling = fields.get(&m.name)?;
            let field = crate::codegen::conventions::field_ident(m, config, super::super::LANG);
            Some(format!("{field}: {source}.{spelling}"))
        })
        .collect();
    format!("&{ty}{{{}}}", parts.join(", "))
}

/// The error-handling block after a call assigns `err_var`: each error the
/// op declares (`@errors`, in declared order) is recognized the way its own
/// `go` block says, a sentinel by identity (`errors.Is`) or a pointer type
/// by type (`errors.As`), and built from the sources the block maps each
/// field to; anything else (including an error with no `go` block, or no
/// declared errors at all) becomes a `ContractError` naming the extern.
/// `ret` turns a built error expression into the caller's own `return`
/// statement (a construction call always returns `nil, err`; an op method
/// returns its own zero value).
#[allow(clippy::too_many_arguments)]
pub(in super::super) fn error_block(
    refs: &mut Vec<Symbol>,
    module: &Module,
    config: &CasingConfig,
    lib: &ExtLib,
    errors: &[String],
    contract_name: &str,
    err_var: &str,
    ret: &dyn Fn(String) -> String,
) -> String {
    let contract = error_names().contract;
    let mut arms = String::new();
    let bindings: Vec<(&String, ForeignLang)> = errors
        .iter()
        .filter_map(|id| ForeignLang::of_error(module, id, "go").map(|fl| (id, fl)))
        .collect();
    if !bindings.is_empty() {
        if let Some(alias) = import_lib(refs, lib) {
            refs.push(import("errors", "errors"));
            for (i, (id, fl)) in bindings.iter().enumerate() {
                let sentinel = qualify(fl.head(), &alias, module);
                if fl.head().starts_with('*') {
                    // A pointer type: matched by type, the matched value
                    // being where the fields come from.
                    let target = format!("{err_var}As{i}");
                    let literal = declared_error_literal(module, config, id, &target, &fl.fields);
                    arms.push_str(&format!(
                        "\tvar {target} {sentinel}\n\tif errors.As({err_var}, &{target}) {{\n\t\t{ret_stmt}\n\t}}\n",
                        ret_stmt = ret(literal),
                    ));
                } else {
                    let literal = declared_error_literal(module, config, id, err_var, &fl.fields);
                    arms.push_str(&format!(
                        "\tif errors.Is({err_var}, {sentinel}) {{\n\t\t{ret_stmt}\n\t}}\n",
                        ret_stmt = ret(literal),
                    ));
                }
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

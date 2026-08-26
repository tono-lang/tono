//! The extern-stub fakes a hermetic declared test needs: the assembly steps
//! that put a fake where each stubbed construction would have stored its
//! value (a decoded literal for a plain field's free-fn stub, a fake handle
//! for a foreign-handle field), the fake handle type itself (one per handle
//! type per test, with one method per declared method, panicking on an
//! unstubbed call so the interface's compiler-enforced method set never
//! falls through silently), and one canned answer as a fake method body.
//! Split out of `vector_tests` to keep it under the file-size ceiling; `use
//! super::*` reaches the parent's `TestCtx` and rendering helpers.

use super::super::entry_field_ident;
use super::*;
use crate::codegen::entries::{call_deps, TailStep};

/// The test's own version of the constructor's tail, as the lines of its
/// assembly closure (two indent levels deep): in the same resolution order,
/// a stubbed free construction assigns its fake instead of calling the
/// resolver (a forwarded handle, which the settings never hold, needs no
/// line at all: its consumer is stubbed too), a field sourced from a handle
/// method calls the same resolver over the fake, and a field that depends
/// on a foreign value renders the same resolution the constructor does. An
/// unstubbed free construction keeps the real call, exactly as the
/// constructor runs it. Returns the preamble statements the fakes need
/// (decoded literals, checked through `t`) alongside the closure lines.
pub(super) fn assembly_steps(
    ctx: &TestCtx<'_>,
    refs: &mut Vec<Symbol>,
    extra_decls: &mut Vec<Decl>,
) -> (String, String) {
    let mut pre = String::new();
    let mut steps = String::new();
    let mut faked: Vec<String> = Vec::new();
    let split = ctx.entry.construction_split(ctx.module);
    for step in &split.tail {
        match step {
            TailStep::Call(f) => {
                let call = f.call.as_ref().expect("a call step carries a call");
                let stub = ctx.test.extern_stubs.iter().find(|s| {
                    matches!(
                        &s.target,
                        ExternStubTarget::Free { lib, fn_ } if *lib == call.ns && *fn_ == call.func
                    )
                });
                let Some(stub) = stub else {
                    steps.push_str(&nest(&resolver_call(ctx, f), 2));
                    continue;
                };
                if ctx.entry.is_forwarded(ctx.module, &f.name) {
                    continue;
                }
                let dest = format!(
                    "s.{}",
                    entry_field_ident(ctx.entry, ctx.module, ctx.config, &f.name)
                );
                match ext::foreign_handle(&f.target, ctx.module) {
                    Some((lib, ty)) => {
                        let handle = lib
                            .types
                            .iter()
                            .find(|t| t.name == ty)
                            .expect("declared handle type");
                        let ident = fake_type_name(ctx, handle);
                        if !faked.contains(&ident) {
                            extra_decls.push(handle_fake_decl(ctx, lib, handle));
                            faked.push(ident.clone());
                        }
                        steps.push_str(&format!("\t\t{dest} = &{ident}{{}}\n"));
                    }
                    None => {
                        let value = stub
                            .answers
                            .first()
                            .and_then(|a| match a {
                                StubAnswer::Value { value } => Some(value.clone()),
                                _ => None,
                            })
                            .unwrap_or(serde_json::Value::Null);
                        push_type_symbols(&f.target, refs);
                        refs.push(import("json", "encoding/json"));
                        let var = format!("{}Stub", camel(&f.name));
                        pre.push_str(&format!(
                            "\tvar {var} {ty}\n\
                             \tif err := json.Unmarshal([]byte({raw}), &{var}); err != nil {{\n\
                             \t\tt.Fatalf(\"decode declared value: %v\", err)\n\t}}\n",
                            ty = go_type(&f.target),
                            raw = go_string(&json_text(&value)),
                        ));
                        steps.push_str(&format!("\t\t{dest} = {var}\n"));
                    }
                }
            }
            TailStep::HandleCall(f) => steps.push_str(&nest(&resolver_call(ctx, f), 2)),
            TailStep::Dependent(f) => {
                let mut helpers = super::super::Helpers::default();
                let mut body = String::new();
                let mut resolve_fns = Vec::new();
                let mut r = super::super::resolve::Resolver {
                    entry: ctx.entry,
                    module: ctx.module,
                    config: ctx.config,
                    helpers: &mut helpers,
                    refs,
                    body: &mut body,
                    resolve_fns: &mut resolve_fns,
                    multi: ctx.multi,
                    fail_value: "nil",
                };
                steps.push_str(&plan::emit_fields_of(
                    std::slice::from_ref(f),
                    ctx.entry,
                    ctx.module,
                    &mut r,
                    2,
                ));
            }
        }
    }
    (pre, steps)
}

/// The resolver call of a foreign step in the test's assembly: the same call
/// the constructor makes, except that a forwarded handle among the
/// dependencies (which the assembly never constructs) is passed as `nil`.
fn resolver_call(ctx: &TestCtx<'_>, field: &crate::ir::EntryField) -> String {
    let args: Vec<String> = call_deps(field)
        .iter()
        .map(|dep| {
            if ctx.entry.is_forwarded(ctx.module, dep) {
                "nil".to_string()
            } else {
                super::super::ext_resolver::dep_arg(ctx.entry, ctx.module, ctx.config, dep)
            }
        })
        .collect();
    super::super::ext_resolver::call_site(
        ctx.entry, ctx.module, ctx.config, ctx.multi, field, &args,
    )
}

/// `depth` tabs on every non-empty line of a column-zero block.
fn nest(block: &str, depth: usize) -> String {
    let pad = "\t".repeat(depth);
    block
        .trim_end_matches('\n')
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                "\n".to_string()
            } else {
                format!("{pad}{l}\n")
            }
        })
        .collect()
}

/// The fake's type name: the test's own function name plus the handle
/// type, so one test faking two handle types declares two distinct types
/// and two tests faking the same handle never collide in the package.
fn fake_type_name(ctx: &TestCtx<'_>, handle: &OpaqueType) -> String {
    format!("{}{}Fake", test_fn_name(ctx), pascal(&handle.name))
}

/// The fake type + methods a handle-method stub needs to satisfy the
/// handle's own generated interface without the real library: the stubbed
/// method returns the canned answer (or the declared error), and any other
/// declared method panics naming the call, so a call this test never
/// expects cannot silently fall through (the interface's method set is
/// compiler-enforced, so every method must exist).
fn handle_fake_decl(ctx: &TestCtx<'_>, lib: &ExtLib, handle: &OpaqueType) -> Decl {
    let type_name = fake_type_name(ctx, handle);
    let mut decl_refs = Vec::new();
    let mut methods = String::new();
    for m in &handle.methods {
        let Some(lang) = ext::go_lang(m) else {
            continue;
        };
        // The same signature the interface and the real adapter render
        // (`ctx context.Context` first for a `ctx`-marked binding), or the
        // fake would not satisfy the interface.
        let (params, ret_ty) = ext::method_signature(m, lang, &mut decl_refs);
        let stubbed = ctx.test.extern_stubs.iter().find(|s| {
            matches!(
                &s.target,
                ExternStubTarget::Method { lib: l, ty, method }
                    if *l == lib.name && *ty == handle.name && *method == m.name
            )
        });
        let body = match stubbed.and_then(|s| s.answers.first()) {
            Some(answer) => fake_method_body(ctx, &m.r#return, answer, &mut decl_refs),
            None => {
                decl_refs.push(import("fmt", "fmt"));
                format!(
                    "\tpanic(fmt.Sprintf({:?}, {:?}))\n",
                    format!(
                        "{}.{}.{}: no stub for this call in test %q",
                        lib.name, handle.name, m.name
                    ),
                    ctx.test.name,
                )
            }
        };
        methods.push_str(&format!(
            "\nfunc (f *{type_name}) {}({}) ({ret_ty}, error) {{\n{body}}}\n",
            lang.symbol,
            params.join(", "),
        ));
    }
    Decl::raw_with(
        format!(
            "// {type_name} fakes the {lib}.{ty} handle for {test:?}: every method the\n\
             // interface declares is implemented, so a call this test never expects\n\
             // fails loudly instead of compiling away silently.\n\
             type {type_name} struct{{}}\n{methods}",
            lib = lib.name,
            ty = handle.name,
            test = ctx.test.name,
        ),
        decl_refs,
    )
}

/// One canned handle-method answer, as a fake method body (no access to
/// `*testing.T`: the fake is a package-level type, not the test function
/// itself, so every branch returns a plain literal or a declared error
/// instead of failing through `t`).
fn fake_method_body(
    ctx: &TestCtx<'_>,
    ret: &Tref,
    answer: &StubAnswer,
    refs: &mut Vec<Symbol>,
) -> String {
    push_type_symbols(ret, refs);
    let zero = format!("\tvar zero {}\n", go_type(ret));
    match answer {
        StubAnswer::Value { value } => {
            refs.push(import("json", "encoding/json"));
            format!(
                "\tvar out {ty}\n\
                 \tif err := json.Unmarshal([]byte({raw}), &out); err != nil {{\n\
                 \t\tpanic(\"decode declared value: \" + err.Error())\n\t}}\n\
                 \treturn out, nil\n",
                ty = go_type(ret),
                raw = go_string(&json_text(value)),
            )
        }
        StubAnswer::Error { error } => format!(
            "{zero}\treturn zero, {}\n",
            declared_error_literal(ctx, &error.shape, &error.data)
        ),
        StubAnswer::Contract { .. } => {
            refs.push(import("errors", "errors"));
            format!("{zero}\treturn zero, errors.New(\"simulated bespoke failure\")\n")
        }
        // Rejected by validation: an http answer never reaches a handle
        // method stub.
        StubAnswer::Http(_) => format!("{zero}\treturn zero, nil\n"),
    }
}

#[cfg(test)]
#[path = "vector_extern_tests.rs"]
mod extern_tests;

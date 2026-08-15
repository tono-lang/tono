//! The extern-stub fakes a hermetic declared test needs: the seam
//! constructor's per-field override arguments (a decoded literal for a
//! plain field's free-fn stub, a fresh fake handle for a foreign-handle
//! field), the fake handle type itself (one method per declared method,
//! panicking on an unstubbed call so the interface's compiler-enforced
//! method set never falls through silently), and one canned answer as a
//! fake method body. Split out of `vector_tests` to keep it under the
//! file-size ceiling; `use super::*` reaches the parent's `TestCtx` and
//! rendering helpers.

use super::*;

/// The seam constructor's per-field override arguments, in the same order
/// [`super::constructor::overridable_fields`] declares them: `nil` for a
/// field nothing stubs (the real call still runs), a pointer to a decoded
/// literal for a plain field's free-fn stub, and a fresh fake handle for a
/// foreign-handle field (built from the matching handle-method stub(s), so
/// the real library is never reached). Returns the preamble statements the
/// non-nil arguments need (temp vars, fake struct construction) alongside
/// the argument expressions themselves.
pub(super) fn extern_override_args(
    ctx: &TestCtx<'_>,
    refs: &mut Vec<Symbol>,
    extra_decls: &mut Vec<Decl>,
) -> (String, Vec<String>) {
    let mut pre = String::new();
    let mut args = Vec::new();
    for f in super::super::constructor::overridable_fields(ctx.entry) {
        let call = f.call.as_ref().expect("overridable field carries a call");
        let stub = ctx.test.extern_stubs.iter().find(|s| {
            matches!(
                &s.target,
                ExternStubTarget::Free { lib, fn_ } if *lib == call.ns && *fn_ == call.func
            )
        });
        let Some(stub) = stub else {
            args.push("nil".to_string());
            continue;
        };
        match ext::foreign_handle(&f.target, ctx.module) {
            Some((lib, ty)) => {
                let handle = lib
                    .types
                    .iter()
                    .find(|t| t.name == ty)
                    .expect("declared handle type");
                let (ident, decl) = handle_fake_decl(ctx, lib, handle);
                extra_decls.push(decl);
                args.push(format!("&{ident}{{}}"));
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
                let var = format!("{}OverrideVal", camel(&f.name));
                pre.push_str(&format!(
                    "\tvar {var} {ty}\n\
                     \tif err := json.Unmarshal([]byte({raw}), &{var}); err != nil {{\n\
                     \t\tt.Fatalf(\"decode declared value: %v\", err)\n\t}}\n",
                    ty = go_type(&f.target),
                    raw = go_string(&json_text(&value)),
                ));
                args.push(format!("&{var}"));
            }
        }
    }
    (pre, args)
}

/// The fake type + methods a handle-method stub needs to satisfy the
/// handle's own generated interface without the real library: the stubbed
/// method returns the canned answer (or the declared error), and any other
/// declared method panics naming the call, so a call this test never
/// expects cannot silently fall through (the interface's method set is
/// compiler-enforced, so every method must exist).
fn handle_fake_decl(ctx: &TestCtx<'_>, lib: &ExtLib, handle: &OpaqueType) -> (String, Decl) {
    let type_name = format!("{}Fake", test_fn_name(ctx));
    let mut decl_refs = Vec::new();
    let mut methods = String::new();
    for m in &handle.methods {
        let Some(lang) = ext::go_lang(m) else {
            continue;
        };
        let params: Vec<String> = m
            .params
            .iter()
            .map(|p| {
                push_type_symbols(&p.r#type, &mut decl_refs);
                format!("{} {}", camel(&p.name), go_type(&p.r#type))
            })
            .collect();
        push_type_symbols(&m.r#return, &mut decl_refs);
        let ret_ty = go_type(&m.r#return);
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
    (
        type_name.clone(),
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
        ),
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

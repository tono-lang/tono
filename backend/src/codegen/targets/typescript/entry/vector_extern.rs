//! The extern-stub swaps a hermetic declared test needs, wrapped around the
//! test body as nested swap/restore pairs over the entry's module-local
//! seams: a free extern-fn stub swaps the field's own seam for its canned
//! logical answer (or, for a foreign-handle field, for a fake handle none of
//! whose methods answer, since every method call the test can reach goes
//! through a seam of its own); a handle-method stub swaps every seam that
//! call feeds, a `= .h.m()` field's seam and/or the called op's own
//! `impl .h.m()` seam. Split out of `vector_tests` to keep it under the
//! file-size ceiling; `use super::*` reaches the parent's `TestCtx` and
//! rendering helpers.

use super::*;
use crate::ir::{EntryField, ExternStub, ExternStubTarget, OpImplCall};

/// Every extern stub this test declares, wrapped as nested swap/restore
/// pairs around `body`: `entries::plan` and `declared_tests::
/// reachable_externs` guarantee every extern call the construction/call
/// path reaches is covered by exactly one of these before generation
/// reaches here, so this only has to spell the swaps themselves, not
/// validate coverage again. A stub that feeds no seam of this entry (a
/// method no field or called op of it reaches) wraps nothing.
pub(super) fn wrap_extern_stubs(ctx: &TestCtx<'_>, body: String, refs: &mut Vec<Symbol>) -> String {
    let mut wrapped = body;
    for stub in &ctx.test.extern_stubs {
        for (swap, answer) in stub_swaps(ctx, stub, refs) {
            refs.push(module_symbol(&swap, ctx.module));
            wrapped = format!(
                "const prev{Swap} = {swap}({answer});\n\
                 try {{\n{inner}}} finally {{\n  {swap}(prev{Swap});\n}}\n",
                Swap = swap.trim_start_matches("swap").trim_end_matches("ForTest"),
                inner = indent(&wrapped, "  "),
            );
        }
    }
    wrapped
}

/// The (swapper, installed value) pairs one stub resolves to against this
/// entry and the test's called op.
fn stub_swaps(
    ctx: &TestCtx<'_>,
    stub: &ExternStub,
    refs: &mut Vec<Symbol>,
) -> Vec<(String, String)> {
    match &stub.target {
        ExternStubTarget::Free { lib, fn_ } => {
            let Some(field) = ctx.entry.fields.iter().find(|f| {
                f.call
                    .as_ref()
                    .is_some_and(|c| &c.ns == lib && &c.func == fn_)
            }) else {
                return Vec::new();
            };
            let answer = if super::super::foreign_handle(&field.target, ctx.module) {
                fake_handle_expr(ctx, &field.target)
            } else {
                free_answer_expr(ctx, &field.target, stub, refs)
            };
            vec![(
                super::super::ext_call::ext_swap_fn_name(ctx.n, field),
                answer,
            )]
        }
        ExternStubTarget::Method { lib, ty, method } => {
            let mut swaps = Vec::new();
            for field in ctx.entry.fields.iter().copied() {
                let Some(call) = &field.handle_call else {
                    continue;
                };
                if call_targets(ctx, call, lib, ty, method) {
                    swaps.push((
                        super::super::ext_call::ext_swap_fn_name(ctx.n, field),
                        method_answer_expr(ctx, call, Some(&field.target), stub, refs),
                    ));
                }
            }
            if let Some(op) = ctx.test.op {
                if let Some(call) = crate::codegen::ops::op_impl_call(op) {
                    if call_targets(ctx, call, lib, ty, method) {
                        let (_, output) = op_io(op);
                        swaps.push((
                            super::super::ext_handle_call::op_swap_fn_name(ctx.n, op),
                            method_answer_expr(ctx, call, output, stub, refs),
                        ));
                    }
                }
            }
            swaps
        }
    }
}

/// Whether a handle-method call site (`.field.method(..)`) reaches the
/// method a stub names: its receiver field's declared type is that lib's
/// handle type and the method name matches.
fn call_targets(ctx: &TestCtx<'_>, call: &OpImplCall, lib: &str, ty: &str, method: &str) -> bool {
    if call.method != method {
        return false;
    }
    let Some(head) = call.recv.first() else {
        return false;
    };
    let Some(field) = ctx.entry.fields.iter().find(|f| f.name == *head) else {
        return false;
    };
    let Tref::Ref { id, .. } = &field.target else {
        return false;
    };
    super::super::ext_handle_iface::resolve_handle(id, ctx.module)
        .is_some_and(|(l, handle)| l.name == lib && handle.name == ty)
}

/// The canned answer of a free extern-fn stub on a plain field, as the
/// value the seam's swap installs: a plain arrow returning the decoded
/// value (the extern's declared `returns:`-projected logical value, or the
/// field's own declared type -- both only ever answer a single `Value`,
/// matching `validate_extern_stub`'s own restriction to `StubAnswer::Value`
/// for a free function).
fn free_answer_expr(
    ctx: &TestCtx<'_>,
    target: &Tref,
    stub: &ExternStub,
    refs: &mut Vec<Symbol>,
) -> String {
    let value = match stub.answers.first() {
        Some(StubAnswer::Value { value }) => value,
        // Rejected by `validate_extern_stub`: a free-fn stub answers a plain
        // value only.
        _ => &serde_json::Value::Null,
    };
    format!(
        "async () => {}",
        decoded_value_expr(target, value, refs, ctx.module)
    )
}

/// The fake a foreign-handle field's free-fn stub installs in place of the
/// real constructor's result: an object satisfying the handle's own
/// generated interface, every declared method throwing naming the call, so
/// a method call this test never expects fails loudly instead of reaching
/// a real library. The methods the test does stub never get here either:
/// each rides its own seam (a `= .h.m()` field's or the op's), which the
/// matching handle-method stub swaps.
fn fake_handle_expr(ctx: &TestCtx<'_>, target: &Tref) -> String {
    let Tref::Ref { id, .. } = target else {
        return "async () => ({})".to_string();
    };
    let Some((lib, handle)) = super::super::ext_handle_iface::resolve_handle(id, ctx.module) else {
        return "async () => ({})".to_string();
    };
    let methods: Vec<String> = handle
        .methods
        .iter()
        .filter_map(|m| {
            let lang = super::super::ext_handle_iface::ts_lang(m)?;
            Some(format!(
                "  {sym}: async () => {{\n    throw new Error({msg});\n  }},\n",
                sym = lang.symbol,
                msg = ts_str(&format!(
                    "{}.{}.{}: no stub for this call in test {:?}",
                    lib.name, handle.name, m.name, ctx.test.name
                )),
            ))
        })
        .collect();
    if methods.is_empty() {
        return "async () => ({})".to_string();
    }
    format!("async () => ({{\n{}}})", methods.join(""))
}

/// The canned answer of a handle-method stub, as the value a seam's swap
/// installs: a plain arrow returning the decoded logical value (the seam's
/// own declared type: the `= .h.m()` field's, or the op's output), or
/// throwing the typed error a declared sentinel of the method's own `ts`
/// binding maps that shape to (the same class the real seam throws for that
/// sentinel). A shape only another language's binding maps has no class in
/// this target, so it throws a plain `Error` naming it. Only the first
/// answer is installed, the same single-answer fake Go's handle fake builds.
fn method_answer_expr(
    ctx: &TestCtx<'_>,
    call: &OpImplCall,
    target: Option<&Tref>,
    stub: &ExternStub,
    refs: &mut Vec<Symbol>,
) -> String {
    match stub.answers.first() {
        Some(StubAnswer::Value { value }) => match target {
            Some(t) => format!(
                "async () => {}",
                decoded_value_expr(t, value, refs, ctx.module)
            ),
            None => "async () => {}".to_string(),
        },
        Some(StubAnswer::Error { error }) => {
            let mapped = ts_method_errors(ctx, call)
                .iter()
                .any(|eb| eb.r#type == error.shape);
            if mapped {
                let class = super::super::ext_call::sentinel_error_class(&error.shape);
                refs.push(module_symbol(&class, ctx.module));
                format!(
                    "async () => {{\n  throw new {class}({});\n}}",
                    json_text(&error.data)
                )
            } else {
                format!(
                    "async () => {{\n  throw new Error({});\n}}",
                    ts_str(&format!("simulated {}", error.shape))
                )
            }
        }
        // Rejected by `validate_extern_stub`: a handle-method stub answers a
        // value or a declared error only.
        _ => "async () => {\n  throw new Error(\"unsupported stub answer\");\n}".to_string(),
    }
}

/// The `errors:` bindings of the `ts` block of the handle method a call
/// site reaches (empty when the method or its `ts` block is missing, which
/// the frontend already rejected).
fn ts_method_errors<'a>(ctx: &TestCtx<'a>, call: &OpImplCall) -> &'a [crate::ir::ErrorBinding] {
    let field: Option<&EntryField> = call
        .recv
        .first()
        .and_then(|head| ctx.entry.fields.iter().find(|f| f.name == *head).copied());
    let Some(Tref::Ref { id, .. }) = field.map(|f| &f.target) else {
        return &[];
    };
    let Some((_, handle)) = super::super::ext_handle_iface::resolve_handle(id, ctx.module) else {
        return &[];
    };
    handle
        .methods
        .iter()
        .find(|m| m.name == call.method)
        .and_then(super::super::ext_handle_iface::ts_lang)
        .map(|lang| lang.errors.as_slice())
        .unwrap_or(&[])
}

//! The extern-stub fakes a hermetic declared test needs: the assembly steps
//! putting a fake where each stubbed construction would have stored its
//! value (a decoded literal for a plain field's free-fn stub, a fake handle
//! for a foreign-handle field), and the fake handle itself, an object
//! satisfying the handle's own generated interface whose stubbed methods
//! answer and whose other methods throw naming the call. Split out of
//! `vector_tests` to keep it under the file-size ceiling; `use super::*`
//! reaches the parent's `TestCtx` and rendering helpers.
//!
//! The handle interface speaks the foreign shape (see `ext_handle_iface`),
//! so a stubbed method answers in that shape: the declared answer is the
//! method's logical value, and a `returns:` projection over field paths is
//! inverted to rebuild the raw object the projection reads (`{ id: a.ID }`
//! answered as `{ ID: ... }`). A projection through a `match` cannot be
//! inverted; `declared_tests::validate_declared_tests` refuses such a stub
//! for this target before generation reaches here.

use super::{decoded_value_expr, indent, json_text, ts_str, TestCtx};
use crate::codegen::entries::{call_deps, TailStep};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::entry::{
    ext_call, ext_handle_iface, ext_resolver, foreign_handle, module_symbol,
};
use crate::ir::{
    EntryField, ExternDecl, ExternStub, ExternStubTarget, ReturnsValue, StubAnswer, Tref,
};

/// The test's own version of the constructor's tail, as the lines of its
/// assembly (indented `prefix`): in the same resolution order, a stubbed
/// free construction assigns its fake instead of calling the resolver (a
/// forwarded handle, which the settings never hold, needs no line at all:
/// its consumer is stubbed too), a field sourced from a handle method calls
/// the same resolver over the fake, and a field that depends on a foreign
/// value renders the same resolution the client does. An unstubbed free
/// construction keeps the real call, exactly as `create` runs it.
pub(super) fn assembly_steps(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>, prefix: &str) -> String {
    let mut steps = String::new();
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
                    steps.push_str(&indent(&resolver_call(ctx, f, refs), prefix));
                    continue;
                };
                if ctx.entry.is_forwarded(ctx.module, &f.name) {
                    continue;
                }
                let dest = super::super::checks::field_path_expr(
                    ctx.entry,
                    ctx.config,
                    std::slice::from_ref(&f.name),
                    "s",
                );
                let value = if foreign_handle(&f.target, ctx.module) {
                    fake_handle_expr(ctx, &f.target, refs)
                } else {
                    free_answer_expr(ctx, &f.target, stub, refs)
                };
                steps.push_str(&indent(&format!("{dest} = {value};\n"), prefix));
            }
            TailStep::HandleCall(f) => {
                steps.push_str(&indent(&resolver_call(ctx, f, refs), prefix))
            }
            TailStep::Dependent(f) => {
                let mut helpers = super::super::Helpers::default();
                let mut body = String::new();
                let mut resolve_fns = Vec::new();
                let mut r = super::super::resolve::Resolver {
                    entry: ctx.entry,
                    module: ctx.module,
                    config: ctx.config,
                    helpers: &mut helpers,
                    body: &mut body,
                    resolve_fns: &mut resolve_fns,
                    multi: !ctx.n.op_prefix.is_empty(),
                    n: ctx.n,
                };
                let rendered = crate::codegen::entries::plan::emit_fields_of(
                    std::slice::from_ref(f),
                    ctx.entry,
                    ctx.module,
                    &mut r,
                    0,
                );
                steps.push_str(&indent(&rendered, prefix));
            }
        }
    }
    steps
}

/// The resolver call of a foreign step in the test's assembly: the same call
/// `create` makes, except that a forwarded handle among the dependencies
/// (which the assembly never constructs) is passed as `undefined`.
fn resolver_call(ctx: &TestCtx<'_>, field: &EntryField, refs: &mut Vec<Symbol>) -> String {
    refs.push(module_symbol(
        &ext_resolver::resolver_name(ctx.n, field),
        ctx.module,
    ));
    let args: Vec<String> = call_deps(field)
        .iter()
        .map(|dep| {
            if ctx.entry.is_forwarded(ctx.module, dep) {
                "undefined as never".to_string()
            } else {
                ext_resolver::dep_arg(ctx.entry, ctx.module, ctx.config, dep)
            }
        })
        .collect();
    ext_resolver::call_site(ctx.entry, ctx.module, ctx.config, ctx.n, field, &args)
}

/// The canned answer of a free extern-fn stub on a plain field: the decoded
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
    decoded_value_expr(target, value, refs, ctx.module)
}

/// The fake a foreign-handle field's free-fn stub assigns in place of the
/// real constructor's result: an object satisfying the handle's own
/// generated interface, every declared method the test stubs answering its
/// canned value (in the foreign shape, see the module doc) and every other
/// one throwing naming the call, so a method call this test never expects
/// fails loudly instead of reaching a real library.
fn fake_handle_expr(ctx: &TestCtx<'_>, target: &Tref, refs: &mut Vec<Symbol>) -> String {
    let Tref::Ref { id, .. } = target else {
        return "{}".to_string();
    };
    let Some((lib, handle)) = ext_handle_iface::resolve_handle(id, ctx.module) else {
        return "{}".to_string();
    };
    let iface = ext_handle_iface::handle_interface_name(id);
    let methods: Vec<String> = handle
        .methods
        .iter()
        .filter_map(|m| {
            let lang = ext_handle_iface::ts_lang(m)?;
            let stub = ctx.test.extern_stubs.iter().find(|s| {
                matches!(
                    &s.target,
                    ExternStubTarget::Method { lib: l, ty, method }
                        if *l == lib.name && *ty == handle.name && *method == m.name
                )
            });
            let body = match stub.and_then(|s| s.answers.first()) {
                Some(answer) => fake_method_body(ctx, m, lang, answer, refs),
                None => format!(
                    "{{\n    throw new Error({msg});\n  }}",
                    msg = ts_str(&format!(
                        "{}.{}.{}: no stub for this call in test {:?}",
                        lib.name, handle.name, m.name, ctx.test.name
                    )),
                ),
            };
            let kw = if m.is_async("ts") { "async " } else { "" };
            Some(format!("  {sym}: {kw}() => {body},\n", sym = lang.symbol))
        })
        .collect();
    if methods.is_empty() {
        return format!("{{}} as {iface}");
    }
    format!("{{\n{}}} as {iface}", methods.join(""))
}

/// One canned handle-method answer, as the fake method's body: the raw
/// value the method's projection reads (see [`raw_answer`]), the typed
/// error a declared sentinel of the method's own `ts` binding maps that
/// shape to (the same class the real call site throws for that sentinel), or
/// a plain `Error` naming a shape only another language's binding maps.
/// Only the first answer is installed, the same single-answer fake Go's
/// handle fake builds.
fn fake_method_body(
    ctx: &TestCtx<'_>,
    method: &ExternDecl,
    lang: &crate::ir::ExternLang,
    answer: &StubAnswer,
    refs: &mut Vec<Symbol>,
) -> String {
    match answer {
        StubAnswer::Value { value } => {
            let raw = raw_answer(lang, value);
            // A foreign-shaped answer is spelled under the companion type the
            // interface declares for it; a logical answer is the op's own
            // value, graded by the fake's own cast to the handle interface.
            let cast = ext_handle_iface::foreign_struct_return(lang, ctx.module)
                .map(|(ty, _)| format!(" as {ty}"))
                .unwrap_or_default();
            format!("({}{cast})", json_literal_ts(&raw))
        }
        StubAnswer::Error { error } => {
            let mapped = method
                .errors
                .iter()
                .any(|id| crate::codegen::entries::local_name(id) == error.shape);
            if mapped {
                let class = ext_call::sentinel_error_class(&error.shape);
                refs.push(module_symbol(&class, ctx.module));
                format!(
                    "{{\n    throw new {class}({});\n  }}",
                    json_text(&error.data)
                )
            } else {
                format!(
                    "{{\n    throw new Error({});\n  }}",
                    ts_str(&format!("simulated {}", error.shape))
                )
            }
        }
        // Rejected by `validate_extern_stub`: a handle-method stub answers a
        // value or a declared error only.
        _ => "{\n    throw new Error(\"unsupported stub answer\");\n  }".to_string(),
    }
}

/// The raw value a fake method answers for a declared logical `value`: the
/// value itself when the method's `ts` binding projects nothing (the raw
/// result already is the logical value), or the object the binding's
/// `returns:` field paths read it out of, rebuilt by inverting each path
/// (`token: cfg.Credentials.Secret` puts the logical `token` under
/// `Credentials.Secret`). A `match` projection has no inverse and is
/// refused before generation.
pub(super) fn raw_answer(
    lang: &crate::ir::ExternLang,
    value: &serde_json::Value,
) -> serde_json::Value {
    let Some(returns) = &lang.returns else {
        return value.clone();
    };
    let mut raw = serde_json::Map::new();
    for field in &returns.fields {
        let ReturnsValue::Field(path) = &field.value else {
            continue;
        };
        let Some(logical) = value.get(&field.name) else {
            continue;
        };
        // The head is the yields name (`raw` itself); the rest is the path
        // into the foreign object.
        let Some((_, rest)) = path.split_first() else {
            continue;
        };
        if rest.is_empty() {
            return logical.clone();
        }
        let mut node = &mut raw;
        for (i, seg) in rest.iter().enumerate() {
            if i + 1 == rest.len() {
                node.insert(seg.clone(), logical.clone());
            } else {
                let next = node
                    .entry(seg.clone())
                    .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
                if !next.is_object() {
                    *next = serde_json::Value::Object(serde_json::Map::new());
                }
                node = next.as_object_mut().expect("just made an object");
            }
        }
    }
    serde_json::Value::Object(raw)
}

/// A JSON value as a TypeScript literal: object keys verbatim (foreign field
/// names ride uncased), the rest in JSON spelling.
fn json_literal_ts(value: &serde_json::Value) -> String {
    ext_call::json_literal(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ReturnsField, ReturnsLit};

    fn lang(returns: Option<ReturnsLit>) -> crate::ir::ExternLang {
        crate::ir::ExternLang {
            lang: "ts".into(),
            symbol: "send".into(),
            call_args: vec![],
            yields: vec![],
            returns,
            chain: None,
        }
    }

    #[test]
    fn a_method_projecting_nothing_answers_the_logical_value_itself() {
        let value = serde_json::json!({"id": "n1", "accepted": true});
        assert_eq!(raw_answer(&lang(None), &value), value);
    }

    #[test]
    fn a_field_projection_is_inverted_into_the_foreign_shape() {
        let returns = ReturnsLit {
            r#type: Tref::Prim(crate::ir::Prim::String),
            fields: vec![
                ReturnsField {
                    name: "id".into(),
                    value: ReturnsValue::Field(vec!["a".into(), "ID".into()]),
                },
                ReturnsField {
                    name: "token".into(),
                    value: ReturnsValue::Field(vec![
                        "a".into(),
                        "Credentials".into(),
                        "Secret".into(),
                    ]),
                },
                ReturnsField {
                    name: "host".into(),
                    value: ReturnsValue::Field(vec![
                        "a".into(),
                        "Credentials".into(),
                        "Host".into(),
                    ]),
                },
            ],
        };
        let value = serde_json::json!({"id": "n1", "token": "s3", "host": "h"});
        assert_eq!(
            raw_answer(&lang(Some(returns)), &value),
            serde_json::json!({"ID": "n1", "Credentials": {"Secret": "s3", "Host": "h"}})
        );
    }

    #[test]
    fn a_projection_of_the_whole_result_answers_the_logical_value() {
        let returns = ReturnsLit {
            r#type: Tref::Prim(crate::ir::Prim::String),
            fields: vec![ReturnsField {
                name: "text".into(),
                value: ReturnsValue::Field(vec!["body".into()]),
            }],
        };
        let value = serde_json::json!({"text": "hello"});
        assert_eq!(
            raw_answer(&lang(Some(returns)), &value),
            serde_json::json!("hello")
        );
    }
}

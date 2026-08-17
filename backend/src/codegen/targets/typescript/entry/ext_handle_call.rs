//! An op's own `impl .field.method(args)` body: a call into an entry
//! field's declared opaque-handle method, in place of the wire protocol.
//!
//! TypeScript has no generated adapter behind a handle-typed field (unlike
//! Go: see `field_ts_type` in `entry/mod.rs`, which renders a foreign
//! handle as `unknown`), so the `yields`/`returns` projection and the
//! sentinel-to-error mapping [`ext_call::call_body`] performs for a free
//! extern-fn call must happen at this call site instead of once behind an
//! interface. This module shares that machinery (`ext_call::render_arg`,
//! `foreign_path_expr`, `arm_value_expr`, `select_expr`,
//! `returns_value_expr`, `sentinel_error_class`) and only replaces the
//! invocation target: a method call on the (narrowed) handle value instead
//! of an imported free function.
//!
//! Calling a method on an `unknown`-typed field needs a narrowing cast; with
//! no generated interface to cast to, the cast is `any`, applied only at
//! this one call site (the field's own declared type stays `unknown`
//! everywhere else).
//!
//! `entries::validate_entries` guarantees, before this runs, that every
//! target in the current generation call supports `emits_ext_handle_calls`
//! (`op_impl_call` would not exist here otherwise); the frontend typechecker
//! is what proves the receiver field and method themselves resolve. The
//! lookups below still fail loudly on a broken invariant rather than
//! silently miscompiling.

use crate::codegen::entries::EntryModel;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::ir::{CallArg, ExternLang, ExternParam, Module, OpImplCall};

use super::checks::field_path_expr;
use super::ext_call::{returns_value_expr, sentinel_error_class};
use super::module_symbol;
use std::collections::BTreeSet;

/// The declared handle method [`impl_call_body`] resolves against: the
/// opaque type's own `ExternDecl` and its `ts`/`typescript` language block.
struct Lookup<'a> {
    lang: &'a ExternLang,
    params: &'a [ExternParam],
}

fn lookup<'a>(module: &'a Module, entry: &EntryModel<'_>, call: &OpImplCall) -> Lookup<'a> {
    let head = call.recv.first().unwrap_or_else(|| {
        panic!("an op's own impl call has no receiver (validate_entries should have rejected this)")
    });
    let field = entry.fields.iter().find(|f| f.name == *head).unwrap_or_else(|| {
        panic!(
            "an op's own impl call names undeclared receiver field {head:?} (the frontend should have rejected this)"
        )
    });
    let crate::ir::Tref::Ref { id: handle_id, .. } = &field.target else {
        panic!(
            "receiver field {:?} of an op's own impl call is not a foreign handle (the frontend should have rejected this)",
            field.name
        )
    };
    let handle = module
        .ext_libs
        .iter()
        .flat_map(|lib| lib.types.iter().map(move |t| (lib, t)))
        .find(|(lib, t)| format!("{}#{}", lib.name, t.name) == *handle_id)
        .map(|(_, t)| t)
        .unwrap_or_else(|| {
            panic!(
                "receiver field {:?} names an unresolved foreign handle {handle_id:?} (the frontend should have rejected this)",
                field.name
            )
        });
    let decl = handle.methods.iter().find(|m| m.name == call.method).unwrap_or_else(|| {
        panic!(
            "an op's own impl call names undeclared method {:?} on handle {:?} (the frontend should have rejected this)",
            call.method, handle.name
        )
    });
    let lang = decl
        .langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
        .unwrap_or_else(|| {
            panic!(
                "handle method {:?} has no typescript binding (validate_entries should have rejected this)",
                call.method
            )
        });
    Lookup {
        lang,
        params: &decl.params,
    }
}

/// [`crate::codegen::targets::typescript::entry::mod::op_method`]'s branch
/// for an operation whose own body is `impl .field.method(args)`: builds the
/// call, projects `yields`/`returns` into the op's own declared output, and
/// maps a declared sentinel (or any unmapped failure) onto the same
/// `ContractError` boundary a free extern call uses.
#[allow(clippy::too_many_arguments)]
pub(super) fn impl_call_body(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    input_name: Option<&str>,
    call: &OpImplCall,
    throw: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
) -> String {
    let l = lookup(module, entry, call);
    let lang = l.lang;

    refs.push(module_symbol(&error_names().contract, module));

    let args = {
        let mut parts = Vec::with_capacity(lang.call_args.len());
        for a in &lang.call_args {
            parts.push(render_call_arg(
                entry, config, a, l.params, &call.args, input_name,
            ));
        }
        parts.join(", ")
    };

    let recv_expr = field_path_expr(entry, config, &call.recv, "this.settings");
    let call_name = format!("{}.{}", call.recv.join("."), call.method);

    // Same `ts` identity as `ext_call.rs`: no yields position ever reads its
    // own `error` slot (a thrown Promise rejection is the only error channel
    // this target has), and `ctx` has no idiomatic TypeScript convention to
    // occupy, so it is ignored the same way Go ignores `sync`.
    let assign = match lang.yields.iter().find(|y| !y.is_error) {
        None => "return raw;".to_string(),
        Some(y) => {
            let returns = lang.returns.as_ref().unwrap_or_else(|| {
                panic!(
                    "handle method {call_name} declares a yields position but no returns to project it into (validate_entries should have rejected this)"
                )
            });
            let projected = returns
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        super::field_camel(&f.name, config),
                        returns_value_expr(&y.name, &f.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("return {{ {projected} }};")
        }
    };

    let mut cases = String::new();
    for eb in &lang.errors {
        let class_name = sentinel_error_class(&eb.r#type);
        sentinel_types.insert(eb.r#type.clone());
        refs.push(module_symbol(&class_name, module));
        cases.push_str(&format!(
            "      case {sentinel:?}: {throw}\n",
            sentinel = eb.sentinel,
            throw = throw(format!("new {class_name}(e)")),
        ));
    }
    let switch = if cases.is_empty() {
        String::new()
    } else {
        format!("  switch (e instanceof Error ? e.message : String(e)) {{\n{cases}  }}\n",)
    };

    let en = error_names();
    format!(
        "  try {{\n    const raw = await (({recv_expr}) as any).{symbol}({args});\n    {assign}\n  }} catch (e) {{\n{switch}    {fallback}\n  }}",
        symbol = lang.symbol,
        fallback = throw(format!("new {}({call_name:?}, e)", en.contract)),
    )
}

/// [`ext_call::render_arg`]'s counterpart for a handle-method call site: a
/// `Ref` here reads either the op's own declared input parameter (its head
/// matching `input_name`, the same recognition Go's `ref_expr` closure in
/// `go/entry/ext.rs::impl_call_body` performs) or a sibling entry field off
/// `this.settings`, instead of `ext_call.rs`'s seam-function parameter `s`.
/// Every other `CallArg` variant matches `ext_call::render_arg` exactly.
fn render_call_arg(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    arg: &CallArg,
    params: &[ExternParam],
    site_args: &[CallArg],
    input_name: Option<&str>,
) -> String {
    match arg {
        CallArg::Param(name) => {
            let idx = params
                .iter()
                .position(|p| &p.name == name)
                .unwrap_or_else(|| {
                    panic!("extern call template references undeclared parameter {name:?}")
                });
            let site = site_args.get(idx).unwrap_or_else(|| {
                panic!("extern call site is missing an argument for parameter {name:?}")
            });
            render_call_arg(entry, config, site, &[], &[], input_name)
        }
        CallArg::Ref(path) => match path.split_first() {
            Some((head, rest)) if Some(head.as_str()) == input_name => {
                if rest.is_empty() {
                    "input".to_string()
                } else {
                    let mut out = "input".to_string();
                    for seg in rest {
                        out.push('.');
                        out.push_str(&super::field_camel(seg, config));
                    }
                    out
                }
            }
            _ => field_path_expr(entry, config, path, "this.settings"),
        },
        CallArg::Lit(v) => super::ext_call::json_literal(v),
        CallArg::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| render_call_arg(entry, config, i, params, site_args, input_name))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Ctor(crate::ir::CallCtor { fields, .. }) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(k, v)| format!(
                    "{k}: {}",
                    render_call_arg(entry, config, v, params, site_args, input_name)
                ))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Call(_) => {
            unimplemented!(
                "a nested extern call used as a handle-method call's argument is not supported yet"
            )
        }
    }
}

#[cfg(test)]
#[path = "ext_handle_call_tests.rs"]
mod tests;

//! An op's own `impl .field.method(args)` body: a call into an entry
//! field's declared opaque-handle method, in place of the wire protocol.
//!
//! TypeScript has no generated adapter behind a handle-typed field (unlike
//! Go: see `ext_handle_iface`, which types a handle field with its own
//! generated interface but never projects a foreign value onto a tono
//! logical type the way Go's adapter does), so the `yields`/`returns`
//! projection and the sentinel-to-error mapping [`ext_call::call_body`]
//! performs for a free extern-fn call must happen at this call site instead
//! of once behind an interface. This module shares that machinery
//! (`ext_call::render_arg`, `foreign_path_expr`, `arm_value_expr`,
//! `select_expr`, `returns_value_expr`, `sentinel_switch`) and only
//! replaces the invocation target: a method call on the receiver field
//! instead of an imported free function.
//!
//! The receiver's own generated interface (`ext_handle_iface`) already
//! types the call with no cast needed at the call site itself. Its own
//! return type is honest, not a guess: `unknown` unless the method's `ts`
//! binding declares a `yields` position naming a foreign struct. A method
//! with no `yields` therefore hands this call site an `unknown` raw
//! result -- true to what tono actually knows -- which this op's own body
//! narrows with `as {op's declared output type}` before returning it. This
//! is not the adapter the interface itself deliberately avoids: it asserts
//! nothing about the *shape* of the value (no field mapping, no
//! projection), only that *this op*, specifically, trusts its own
//! frontend-checked contract that a handle method with no yields hands back
//! exactly what the op declared.
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
use crate::ir::{ExternLang, ExternParam, Module, OpImplCall};

use super::checks::field_path_expr;
use super::ext_call::{render_arg, returns_value_expr, sentinel_switch};
use super::ext_handle_iface::resolve_handle;
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
    let (_lib, handle) = resolve_handle(handle_id, module).unwrap_or_else(|| {
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
    ret: &str,
    throw: &dyn Fn(String) -> String,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
) -> String {
    let l = lookup(module, entry, call);
    let lang = l.lang;

    refs.push(module_symbol(&error_names().contract, module));

    let ref_expr =
        |entry: &EntryModel<'_>, config: &crate::codegen::casing::CasingConfig, path: &[String]| {
            handle_call_ref_expr(entry, config, path, input_name)
        };
    let args = {
        let mut parts = Vec::with_capacity(lang.call_args.len());
        for a in &lang.call_args {
            parts.push(render_arg(
                entry, config, a, l.params, &call.args, &ref_expr,
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
        // No yields: the interface honestly types `raw` as `unknown` (see
        // the module doc for why). `void` accepts any resolved value as-is;
        // any other declared output narrows through the op's own return
        // type, trusting the frontend-checked contract that this op's
        // impl call hands back exactly what it declared, not a guess this
        // module invents.
        None if ret == "void" => "return raw;".to_string(),
        None => format!("return raw as {ret};"),
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

    let switch = sentinel_switch(&lang.errors, module, refs, sentinel_types, throw);

    let en = error_names();
    format!(
        "  try {{\n    const raw = await {recv_expr}.{symbol}({args});\n    {assign}\n  }} catch (e) {{\n{switch}    {fallback}\n  }}",
        symbol = lang.symbol,
        fallback = throw(format!("new {}({call_name:?}, e)", en.contract)),
    )
}

/// [`ext_call::render_arg`]'s `Ref` resolver for a handle-method call site:
/// the op's own declared input parameter (its head matching `input_name`,
/// the same recognition Go's `ref_expr` closure in
/// `go/entry/ext.rs::impl_call_body` performs) or a sibling entry field off
/// `this.settings`, instead of `ext_call.rs`'s default (a sibling field off
/// the seam function's own parameter `s`).
fn handle_call_ref_expr(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    path: &[String],
    input_name: Option<&str>,
) -> String {
    match path.split_first() {
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
    }
}

#[cfg(test)]
#[path = "ext_handle_call_tests.rs"]
mod tests;

//! A call into an entry field's declared opaque-handle method: an op's own
//! `impl .field.method(args)` body (in place of the wire protocol), or a
//! field's own `= .field.method(args)` construction source (a foreign
//! resolution that feeds several operations).
//!
//! TypeScript has no generated adapter behind a handle-typed field (unlike
//! Go: see `ext_handle_iface`, which types a handle field with its own
//! generated interface but never projects a foreign value onto a tono
//! logical type the way Go's adapter does), so the `yields`/`returns`
//! projection and the sentinel-to-error mapping [`ext_call::call_body`]
//! performs for a free extern-fn call must happen at this call site instead
//! of once behind an interface. That is also why a declared test's fake
//! handle answers in the foreign shape (`vector_extern`): the projection
//! runs over whatever the handle returns, real or fake. This module shares that machinery
//! (`ext_call::render_arg`, `foreign_path_expr`, `arm_value_expr`,
//! `select_expr`, `returns_value_expr`, `sentinel_switch`) and only
//! replaces the invocation target: a method call on the receiver field
//! instead of an imported free function.
//!
//! The receiver's own generated interface (`ext_handle_iface`) already
//! types the call with no cast needed at the call site itself: a method
//! whose `ts` binding declares a `yields` position naming a foreign struct
//! answers that struct's verbatim shape, which the projection below reads
//! field by field; any other method answers the return it declares, so the
//! raw result already is the value this site returns, and the target
//! compiler grades that the declared type and the library's agree.
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
use crate::ir::{ExtLib, ExternLang, ExternParam, Module, OpImplCall};

use super::checks::field_path_expr;
use super::ext_call::{class_reference_imports, render_arg, returns_value_expr, sentinel_switch};
use super::ext_handle_iface::resolve_handle;
use super::module_symbol;
use std::collections::BTreeSet;

/// The declared handle method [`impl_call_body`] resolves against: the
/// opaque type's own `ExternDecl` and its `ts`/`typescript` language block.
struct Lookup<'a> {
    lib: &'a ExtLib,
    decl: &'a crate::ir::ExternDecl,
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
    let (lib, handle) = resolve_handle(handle_id, module).unwrap_or_else(|| {
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
        lib,
        lang,
        decl,
        params: &decl.params,
    }
}

/// The pieces of one handle-method call site that differ between an op's
/// own `impl` body and a field's own `= .h.m(args)` source: where the
/// receiver is read from, how a `Ref` argument resolves, and how a failure
/// leaves the block.
struct CallSite<'a> {
    recv_expr: String,
    ref_expr:
        &'a dyn Fn(&EntryModel<'_>, &crate::codegen::casing::CasingConfig, &[String]) -> String,
    throw: &'a dyn Fn(String) -> String,
}

/// The `try`/`catch` shape both call sites share: the call, its
/// `yields`/`returns` projection (or the raw pass-through), and the sentinel
/// mapping onto the same `ContractError` boundary a free extern call uses.
/// Returns the four rendered parts (`recv.symbol(args)` expression, the
/// `return ...;` statement, the sentinel switch, and the fallback throw)
/// so each caller lays them out with its own indentation.
#[allow(clippy::too_many_arguments)]
fn call_parts(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    call: &OpImplCall,
    site: &CallSite<'_>,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
) -> (String, String, String, String) {
    let l = lookup(module, entry, call);
    let lang = l.lang;

    refs.push(module_symbol(&error_names().contract, module));
    class_reference_imports(&lang.call_args, l.lib, refs);

    let args = {
        let mut parts = Vec::with_capacity(lang.call_args.len());
        for a in &lang.call_args {
            parts.push(render_arg(
                entry,
                config,
                module,
                l.lib,
                a,
                l.params,
                &call.args,
                site.ref_expr,
            ));
        }
        parts.join(", ")
    };

    let recv_expr = &site.recv_expr;
    let call_name = format!("{}.{}", call.recv.join("."), call.method);

    // Same `ts` identity as `ext_call.rs`: no yields position ever reads its
    // own `error` slot (a thrown Promise rejection is the only error channel
    // this target has), and `ctx` has no idiomatic TypeScript convention to
    // occupy, so it is ignored the same way Go ignores `sync`.
    let assign = match (
        lang.yields.iter().find(|y| !y.is_error),
        lang.returns.as_ref(),
    ) {
        // No projection: the interface types `raw` as the method's declared
        // return (see the module doc), whether the binding left the
        // positions to the convention or named the one it returns, so the
        // raw result already is this site's value.
        (_, None) => "return raw;".to_string(),
        (None, Some(_)) => panic!(
            "handle method {call_name} declares a returns but no yields position to project from (validate_entries should have rejected this)"
        ),
        (Some(y), Some(returns)) => {
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

    let switch = sentinel_switch(
        &l.decl.errors,
        l.lib,
        module,
        refs,
        sentinel_types,
        site.throw,
    );
    let en = error_names();
    let fallback = (site.throw)(format!("new {}({call_name:?}, e)", en.contract));
    (
        format!("{recv_expr}.{}({args})", lang.symbol),
        assign,
        switch,
        fallback,
    )
}

/// The body of an operation whose own implementation is `impl
/// .field.method(args)`, inline in the generated method: builds the call
/// off the client's resolved settings (`root`, `this.settings`), projects
/// `yields`/`returns` into the op's own declared output, and maps a
/// declared sentinel (or any unmapped failure) onto the same
/// `ContractError` boundary a free extern call uses. A declared test stubs
/// the handle itself (a fake whose methods answer), never this call.
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
    root: &str,
) -> String {
    let ref_expr =
        |entry: &EntryModel<'_>, config: &crate::codegen::casing::CasingConfig, path: &[String]| {
            handle_call_ref_expr(entry, config, path, input_name, root)
        };
    let site = CallSite {
        recv_expr: field_path_expr(entry, config, &call.recv, root),
        ref_expr: &ref_expr,
        throw,
    };
    let (call_expr, assign, switch, fallback) =
        call_parts(entry, config, module, call, &site, refs, sentinel_types);
    format!(
        "  try {{\n    const raw = await {call_expr};\n    {assign}\n  }} catch (e) {{\n{switch}    {fallback}\n  }}"
    )
}

/// A field's own `= .field.method(args)` construction source, as the body
/// of that field's resolver (see `ext_resolver`): the receiver and every
/// sibling field a `Ref` argument reads are the resolver's own parameters
/// (`recv_expr`, `ref_expr`). Ends in `return`: the assignment itself lives
/// at the field's own resolution point.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_call_body(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    call: &OpImplCall,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
    recv_expr: &str,
    ref_expr: &dyn Fn(&EntryModel<'_>, &crate::codegen::casing::CasingConfig, &[String]) -> String,
) -> String {
    let throw = |expr: String| format!("throw {expr};");
    let site = CallSite {
        recv_expr: recv_expr.to_string(),
        ref_expr,
        throw: &throw,
    };
    let (call_expr, assign, switch, fallback) =
        call_parts(entry, config, module, call, &site, refs, sentinel_types);
    format!("try {{\n  const raw = await {call_expr};\n  {assign}\n}} catch (e) {{\n{switch}  {fallback}\n}}")
}

/// [`ext_call::render_arg`]'s `Ref` resolver for an op's own handle-method
/// call site: the op's own declared input parameter (its head matching
/// `input_name`, the same recognition Go's `ref_expr` closure in
/// `go/entry/ext.rs::impl_call_body` performs) or a sibling entry field off
/// the seam's resolved `Settings` `s` (`ext_call.rs`'s default, reached
/// through `field_path_expr` here since the head may also be the input).
fn handle_call_ref_expr(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    path: &[String],
    input_name: Option<&str>,
    root: &str,
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
        _ => field_path_expr(entry, config, path, root),
    }
}

#[cfg(test)]
#[path = "ext_handle_call_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ext_handle_source_tests.rs"]
mod source_tests;

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

use crate::codegen::entries::{op_local_name, EntryModel};
use crate::codegen::ops::{error_names, input_name, op_impl_call, op_io, wire_binding};
use crate::codegen::symbol::Symbol;
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::type_expr_of;
use crate::codegen::tree::Decl;
use crate::ir::{ExtLib, ExternLang, ExternParam, Module, OpImplCall, Shape};

use super::checks::field_path_expr;
use super::ext_call::{class_reference_imports, render_arg, returns_value_expr, sentinel_switch};
use super::ext_handle_iface::resolve_handle;
use super::module_symbol;
use std::collections::BTreeSet;

/// The declared handle method [`impl_call_body`] resolves against: the
/// opaque type's own `ExternDecl` and its `ts`/`typescript` language block.
struct Lookup<'a> {
    lib: &'a ExtLib,
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
        params: &decl.params,
    }
}

/// The pieces of one handle-method call site that differ between an op's
/// own `impl` body and a field's own `= .h.m(args)` source: where the
/// receiver is read from, how a `Ref` argument resolves, what the raw
/// result is narrowed to when the method declares no `yields`, and how a
/// failure leaves the block.
struct CallSite<'a> {
    recv_root: &'a str,
    ref_expr:
        &'a dyn Fn(&EntryModel<'_>, &crate::codegen::casing::CasingConfig, &[String]) -> String,
    ret: &'a str,
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
                l.lib,
                a,
                l.params,
                &call.args,
                site.ref_expr,
            ));
        }
        parts.join(", ")
    };

    let recv_expr = field_path_expr(entry, config, &call.recv, site.recv_root);
    let call_name = format!("{}.{}", call.recv.join("."), call.method);

    // Same `ts` identity as `ext_call.rs`: no yields position ever reads its
    // own `error` slot (a thrown Promise rejection is the only error channel
    // this target has), and `ctx` has no idiomatic TypeScript convention to
    // occupy, so it is ignored the same way Go ignores `sync`.
    let assign = match lang.yields.iter().find(|y| !y.is_error) {
        // No yields: the interface honestly types `raw` as `unknown` (see
        // the module doc for why). `void` accepts any resolved value as-is;
        // any other declared type narrows through the site's own return
        // type, trusting the frontend-checked contract that this call hands
        // back exactly what was declared, not a guess this module invents.
        None if site.ret == "void" => "return raw;".to_string(),
        None => format!("return raw as {};", site.ret),
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

    let switch = sentinel_switch(&lang.errors, module, refs, sentinel_types, site.throw);
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
/// .field.method(args)`, as the module-scoped seam the generated method
/// goes through ([`op_seam_decls`]): builds the call off the resolved
/// `Settings` `s` (the seam's own parameter, the same root a field's
/// `= .h.m()` seam reads), projects `yields`/`returns` into the op's own
/// declared output, and maps a declared sentinel (or any unmapped failure)
/// onto the same `ContractError` boundary a free extern call uses.
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
    let ref_expr =
        |entry: &EntryModel<'_>, config: &crate::codegen::casing::CasingConfig, path: &[String]| {
            handle_call_ref_expr(entry, config, path, input_name)
        };
    let site = CallSite {
        recv_root: "s",
        ref_expr: &ref_expr,
        ret,
        throw,
    };
    let (call_expr, assign, switch, fallback) =
        call_parts(entry, config, module, call, &site, refs, sentinel_types);
    format!(
        "  try {{\n    const raw = await {call_expr};\n    {assign}\n  }} catch (e) {{\n{switch}    {fallback}\n  }}"
    )
}

/// The module-local binding an `impl .field.method(args)` operation's
/// generated method calls instead of reaching the handle inline: the same
/// mutable indirection a bespoke `ext impl` op ([`super::impl_op`]) and a
/// free extern-fn field ([`super::ext_call`]) already have, so a declared
/// test can stub the handle method by swapping this one binding without a
/// fake handle whose foreign raw shape it would have to invent.
pub(super) fn op_seam_var(n: &super::Names, op: &Shape) -> String {
    super::camel(&format!(
        "{}{}_handle_call",
        n.op_prefix,
        op_local_name(&op.id)
    ))
}

/// The exported swapper a generated test simulates a handle-method stub
/// through, for one operation's seam.
pub(super) fn op_swap_fn_name(n: &super::Names, op: &Shape) -> String {
    format!(
        "swap{}HandleCallForTest",
        super::pascal(&format!("{}{}", n.op_prefix, op_local_name(&op.id)))
    )
}

/// The per-operation seam bindings of an entry's `impl .field.method(args)`
/// operations: one mutable `let` per such op (an async function taking the
/// resolved `Settings` and the op's own input, performing the real
/// call/projection/error-mapping), plus (only when the entry declares tests,
/// so the shipped surface stays clean without opt-in) the exported swapper
/// the generated tests go through.
pub(super) fn op_seam_decls(
    entry: &EntryModel<'_>,
    n: &super::Names,
    module: &Module,
    config: &crate::codegen::casing::CasingConfig,
    helpers: &mut super::Helpers,
    has_tests: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    for op in entry.operations {
        if wire_binding(op).is_some() {
            continue;
        }
        let Some(call) = op_impl_call(op) else {
            continue;
        };
        let (input, output) = op_io(op);
        let mut refs = Vec::new();
        let params = match input {
            Some(t) => {
                refs.extend(super::type_refs(t, module));
                format!(
                    "s: {}, input: {}",
                    n.settings,
                    render_type(&type_expr_of(t), &TsRules)
                )
            }
            None => format!("s: {}", n.settings),
        };
        let arg_names = if input.is_some() { "s, input" } else { "s" };
        if let Some(t) = output {
            refs.extend(super::type_refs(t, module));
        }
        let ret = output
            .map(|t| render_type(&type_expr_of(t), &TsRules))
            .unwrap_or_else(|| "void".to_string());
        let throw = |expr: String| format!("throw {expr};");
        let body = impl_call_body(
            entry,
            config,
            module,
            input_name(op),
            call,
            &ret,
            &throw,
            &mut refs,
            &mut helpers.ext_error_types,
        );
        let seam = op_seam_var(n, op);
        let call_name = format!(".{}.{}", call.recv.join("."), call.method);
        let mut text = format!(
            "// {seam} is the call the generated method goes through to reach\n\
             // {call_name} on the resolved handle (a test that simulates an outcome swaps\n\
             // this module-local one instead).\n\
             let {seam}: ({params}) => Promise<{ret}> = async ({arg_names}) => {{\n{body}\n}};",
        );
        if has_tests {
            text.push_str(&format!(
                "\n\n// {swap} swaps the handle-method call {local} goes through and returns the\n\
                 // previous one; a test generated from the declared tests simulates an\n\
                 // outcome with it and restores the real call afterwards.\n\
                 export function {swap}(next: typeof {seam}): typeof {seam} {{\n\
                 \x20 const prev = {seam};\n\
                 \x20 {seam} = next;\n\
                 \x20 return prev;\n\
                 }}",
                swap = op_swap_fn_name(n, op),
                local = op_local_name(&op.id),
            ));
        }
        decls.push(Decl::raw_with(text, refs));
    }
    decls
}

/// A field's own `= .field.method(args)` construction source, as the body
/// of that field's module-scoped seam (see `ext_call::seam_decls`): the
/// receiver is read off the in-progress `Settings` draft `s` (the handle
/// field resolved earlier in the same constructor), a `Ref` argument is a
/// sibling field off the same draft, and the raw result narrows to the
/// field's own declared type. Ends in `return`, like a free call's seam
/// body: the assignment itself lives at the field's own resolution point.
#[allow(clippy::too_many_arguments)]
pub(super) fn handle_call_body(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    field: &crate::ir::EntryField,
    call: &OpImplCall,
    refs: &mut Vec<Symbol>,
    sentinel_types: &mut BTreeSet<String>,
) -> String {
    let ret = super::field_ts_type(&field.target, module);
    let throw = |expr: String| format!("throw {expr};");
    let site = CallSite {
        recv_root: "s",
        ref_expr: &super::ext_call::field_ref,
        ret: &ret,
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
        _ => field_path_expr(entry, config, path, "s"),
    }
}

#[cfg(test)]
#[path = "ext_handle_call_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ext_handle_source_tests.rs"]
mod source_tests;

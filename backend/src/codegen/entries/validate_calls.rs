//! Generation-time resolution of a field's own extern call (`= ns.fn(args)`
//! and `= .field.method(args)`) against the module's `ext_libs`: the callee
//! exists, is bound for every target being generated, projects what it
//! yields, and does not forward a handle no emitter can unwrap. Split out
//! of `validate.rs` to stay under the per-file line ceiling; the driver
//! (`validate::validate_entries`) calls these per field.

use super::validate::{is_foreign_ref, ref_id, ref_paths};
use crate::codegen::output::TargetKind;
use crate::ir::{EntryField, ExternDecl, Module, OpImplCall};

/// A field's own `= ns.fn(args)` call cannot forward a sibling field that is
/// itself an *injected* (not tono-constructed) foreign handle. A handle
/// field this generator constructed (`field.call`) always stores its result
/// wrapped in the generated adapter, so a target's own emitter can unwrap it
/// back to the real value another foreign call declares its parameter as
/// (see `go/entry/ext.rs::sibling_path_expr`). A handle field sourced from
/// `@arg`/`@with` instead holds whatever value the caller supplied, which is
/// not guaranteed to be that adapter (or even the real library's own
/// concrete type) at all, so no target's emitter can safely unwrap it: the
/// combination is rejected here, at generation time, naming both fields and
/// the call, rather than reaching the target compiler as an error about a
/// type the author never wrote (or, if a target's emitter were to guess at
/// unwrapping it anyway, panicking at runtime on a valid caller-supplied
/// value).
pub(super) fn injected_handle_forwarded_to_another_call(
    module: &Module,
    declared: &[&EntryField],
    field: &EntryField,
) -> Result<(), String> {
    let (site, args) = match (&field.call, &field.handle_call) {
        (Some(call), _) => (
            format!("{} = {}.{}(..)", field.name, call.ns, call.func),
            call.args.as_slice(),
        ),
        (None, Some(call)) => (
            format!(
                "{} = .{}.{}(..)",
                field.name,
                call.recv.join("."),
                call.method
            ),
            call.args.as_slice(),
        ),
        (None, None) => return Ok(()),
    };
    let mut paths = Vec::new();
    ref_paths(args, &mut paths);
    for path in paths {
        let Some(head) = path.first() else { continue };
        let Some(sibling) = declared.iter().find(|f| f.name == *head) else {
            continue;
        };
        let is_handle = ref_id(&sibling.target).is_some_and(|id| is_foreign_ref(module, id));
        // A handle field with no free construction call is injected: a
        // handle-method source never produces a handle-typed field
        // (`handle_call_resolves` rejects that first), so `call` alone
        // decides.
        if is_handle && sibling.call.is_none() {
            return Err(format!(
                "{site}: argument {head:?} names field {}, an injected foreign handle with no construction call of its own; only a handle this generator itself constructed can be forwarded into another extern call",
                sibling.name
            ));
        }
    }
    Ok(())
}

/// A field's `= ns.fn(args)` source must resolve to a declared `extern` that
/// carries a binding for every target being generated. Without this an emitter
/// would meet "no such ext block" or "no block for my language" as a pipeline
/// defect -- a panic -- instead of an authoring error carrying a message.
pub(super) fn call_resolves(
    module: &Module,
    field: &EntryField,
    targets: &[TargetKind],
) -> Result<(), String> {
    let Some(call) = &field.call else {
        return Ok(());
    };
    let site = format!("{} = {}.{}(..)", field.name, call.ns, call.func);
    let Some(lib) = module.ext_libs.iter().find(|l| l.name == call.ns) else {
        return Err(format!(
            "{site}: no ext block named {} is declared in this module",
            call.ns
        ));
    };
    let Some(decl) = lib.externs.iter().find(|e| e.name == call.func) else {
        return Err(format!(
            "{site}: ext {} declares no extern named {}",
            call.ns, call.func
        ));
    };
    extern_binds_every_target(&site, &call.func, decl, targets)
}

/// A field's `= .field.method(args)` source must name a sibling foreign
/// handle field and one of its declared methods, bound for every target
/// being generated: the same resolution [`call_resolves`] performs for a
/// free call, against the handle's method table instead of the ext block's
/// free externs. The frontend already resolves receiver and method, but the
/// IR is also accepted straight from a file, so the gap surfaces here as an
/// authoring error rather than a panic inside an emitter.
pub(super) fn handle_call_resolves(
    module: &Module,
    declared: &[&EntryField],
    field: &EntryField,
    targets: &[TargetKind],
) -> Result<(), String> {
    let Some(call) = &field.handle_call else {
        return Ok(());
    };
    let site = format!(
        "{} = .{}.{}(..)",
        field.name,
        call.recv.join("."),
        call.method
    );
    let decl = handle_method_of(module, declared, call, &site)?;
    // A method's declared return is spelled through the same projection an
    // op's own `impl` output is (a logical wire type); no target spells a
    // foreign handle in that position, so a field would compile against a
    // type its emitter never named. Rejected here rather than at the
    // target compiler.
    if ref_id(&field.target).is_some_and(|id| is_foreign_ref(module, id)) {
        return Err(format!(
            "{site}: the field is itself a foreign handle; a handle method call only sources a field of a logical (non-handle) type, construct a handle with a free extern call instead"
        ));
    }
    extern_binds_every_target(&site, &call.method, decl, targets)
}

/// An operation's own `impl .field.method(args)` body must name a foreign
/// handle field of its entry and one of its declared methods, bound for
/// every target being generated: the same resolution
/// [`handle_call_resolves`] performs for a field's `= .field.method(args)`
/// source, at the op's own call site. Without it a target whose binding the
/// method lacks would meet "no block for my language" inside its emitter as
/// a pipeline defect (a panic) instead of an authoring error naming the op.
pub(super) fn op_impl_call_resolves(
    module: &Module,
    declared: &[&EntryField],
    op_name: &str,
    call: &OpImplCall,
    targets: &[TargetKind],
) -> Result<(), String> {
    let site = format!(
        "operation {op_name} impl .{}.{}(..)",
        call.recv.join("."),
        call.method
    );
    let decl = handle_method_of(module, declared, call, &site)?;
    extern_binds_every_target(&site, &call.method, decl, targets)
}

/// The declared handle method a `.field.method(args)` call site (a field
/// source or an op's own body) reaches: the receiver names a foreign-handle
/// field of the entry, and the method one of that handle's own.
fn handle_method_of<'m>(
    module: &'m Module,
    declared: &[&EntryField],
    call: &OpImplCall,
    site: &str,
) -> Result<&'m ExternDecl, String> {
    let Some(head) = call.recv.first() else {
        return Err(format!("{site}: the handle method call has no receiver"));
    };
    let Some(recv) = declared.iter().find(|f| f.name == *head) else {
        return Err(format!(
            "{site}: receiver {head} is not a field of this entry"
        ));
    };
    let handle = ref_id(&recv.target).and_then(|id| {
        let (lib_name, type_name) = id.split_once('#')?;
        let lib = module.ext_libs.iter().find(|l| l.name == lib_name)?;
        lib.types.iter().find(|t| t.name == type_name)
    });
    let Some(handle) = handle else {
        return Err(format!(
            "{site}: receiver {head} is not a foreign handle field; a handle method call reads a method of a field typed by an ext block's opaque type"
        ));
    };
    handle
        .methods
        .iter()
        .find(|m| m.name == call.method)
        .ok_or_else(|| {
            format!(
                "{site}: handle {} declares no method named {}",
                handle.name, call.method
            )
        })
}

/// The per-target half of resolving an extern call (free or method): a
/// language block per target, `yields` projected by a `returns`, and the
/// single-result rule TypeScript's call convention imposes.
fn extern_binds_every_target(
    site: &str,
    callee: &str,
    decl: &ExternDecl,
    targets: &[TargetKind],
) -> Result<(), String> {
    for target in targets {
        let Some(lang) = decl
            .langs
            .iter()
            .find(|l| target.binding_langs().contains(&l.lang.as_str()))
        else {
            return Err(format!(
                "{site}: extern {callee} declares no {} block; {} codegen has nothing to emit",
                target.binding_langs()[0],
                target.dir()
            ));
        };
        // A hand-fed IR bypasses the frontend's own "every yields: position
        // is consumed" rule (`tono gen` accepts raw IR directly), so this
        // reasserts it at generation time: a non-error position with
        // nothing declared to project it into would meet an emitter as a
        // pipeline defect instead of an authoring error.
        let projects = lang.yields.iter().any(|y| !y.is_error);
        if projects && lang.returns.is_none() {
            return Err(format!(
                "{site}: the {} block declares yields but no returns to project them into",
                target.binding_langs()[0],
            ));
        }
        // A single call expression can only ever produce one value in
        // TypeScript (no multi-return), so a binding naming more than one
        // non-error position has nothing to spell it as, unlike a target
        // whose call convention returns several values at once.
        if *target == TargetKind::TypeScript {
            let non_error = lang.yields.iter().filter(|y| !y.is_error).count();
            if non_error > 1 {
                return Err(format!(
                    "{site}: the {} block names {non_error} non-error yields positions; a single TypeScript call result can only bind one",
                    target.binding_langs()[0],
                ));
            }
        }
        static_receiver_renders(site, *target, lang)?;
        class_reference_renders(site, *target, lang)?;
        if !target.emits_nested_extern_call_args() && contains_cross_extern_call(&lang.call_args) {
            return Err(format!(
                "{site}: the {} block's call: line uses another declared extern's call as one of its own arguments; {} codegen cannot render that yet",
                target.binding_langs()[0],
                target.dir()
            ));
        }
    }
    Ok(())
}

/// A `call:` line whose receiver is a foreign type name (a static method,
/// `"Type"."method"(args)`) is rejected for a target that has nothing to
/// spell it as (see [`TargetKind::emits_static_receiver_calls`]), naming
/// the site and the method, before any emitter could write a method
/// expression that compiles into the wrong call.
pub(super) fn static_receiver_renders(
    site: &str,
    target: TargetKind,
    lang: &crate::ir::ExternLang,
) -> Result<(), String> {
    match &lang.receiver {
        Some(recv) if !target.emits_static_receiver_calls() => Err(format!(
            "{site}: the {} block's call: line is a static method of the foreign type {recv:?} ({recv}.{}); {} has no static method to call, bind a package function instead",
            target.binding_langs()[0],
            lang.symbol,
            target.dir()
        )),
        _ => Ok(()),
    }
}

/// A `call:` line passing a class reference (`type handle`, the declared
/// handle's foreign type itself, for a library that constructs it) is
/// rejected for a target that has no type as a value to pass (see
/// [`TargetKind::emits_class_reference_args`]), naming the site and the
/// handle, before any emitter could spell a type where a value goes.
pub(super) fn class_reference_renders(
    site: &str,
    target: TargetKind,
    lang: &crate::ir::ExternLang,
) -> Result<(), String> {
    match first_class_reference(&lang.call_args) {
        Some(handle) if !target.emits_class_reference_args() => Err(format!(
            "{site}: the {} block's call: line passes the class of handle {handle:?} as an argument (type {handle}); {} has no class reference to pass, bind a function that constructs it instead",
            target.binding_langs()[0],
            target.dir()
        )),
        _ => Ok(()),
    }
}

/// A wire-position call (`@header`/`@body` value) passes the trait's own
/// arguments positionally and never applies the binding's `call:` argument
/// template, so a class reference written there would be dropped silently
/// in every target; refused by name instead, whatever the target.
pub(super) fn class_reference_in_wire_position(
    site: &str,
    target: TargetKind,
    lang: &crate::ir::ExternLang,
) -> Result<(), String> {
    match first_class_reference(&lang.call_args) {
        Some(handle) => Err(format!(
            "{site}: the {} block's call: line passes the class of handle {handle:?} as an argument (type {handle}); a call in wire position passes the trait's own arguments and cannot carry a class reference",
            target.binding_langs()[0],
        )),
        None => Ok(()),
    }
}

/// The first class reference anywhere in a call's own argument tree, walked
/// through the same nesting shapes [`ref_paths`] does.
fn first_class_reference(args: &[crate::ir::CallArg]) -> Option<&str> {
    use crate::ir::CallArg;
    args.iter().find_map(|a| match a {
        CallArg::TypeRef(handle) => Some(handle.as_str()),
        CallArg::Ctor(ctor) => {
            let fields: Vec<&CallArg> = ctor.fields.values().collect();
            fields
                .iter()
                .find_map(|v| first_class_reference(std::slice::from_ref(*v)))
        }
        CallArg::List(items) => first_class_reference(items),
        CallArg::SymbolCall(sc) => first_class_reference(&sc.args),
        CallArg::Param(_) | CallArg::Ref(_) | CallArg::Lit(_) | CallArg::Call(_) => None,
    })
}

/// Whether a call's own argument tree uses a cross-extern call
/// (`CallArg::Call`, resolved against a declared `extern`) anywhere, walked
/// through the same nesting shapes [`ref_paths`] does (a ctor field's own
/// value, a list's items) -- the only positions the surface grammar ever
/// produces one in.
fn contains_cross_extern_call(args: &[crate::ir::CallArg]) -> bool {
    use crate::ir::CallArg;
    args.iter().any(|a| match a {
        CallArg::Call(_) => true,
        CallArg::Ctor(ctor) => ctor.fields.values().any(contains_cross_extern_call_one),
        CallArg::List(items) => contains_cross_extern_call(items),
        CallArg::SymbolCall(sc) => contains_cross_extern_call(&sc.args),
        CallArg::Param(_) | CallArg::Ref(_) | CallArg::Lit(_) | CallArg::TypeRef(_) => false,
    })
}

fn contains_cross_extern_call_one(arg: &crate::ir::CallArg) -> bool {
    contains_cross_extern_call(std::slice::from_ref(arg))
}

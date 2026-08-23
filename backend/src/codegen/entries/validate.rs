//! Generation-time validation of the entry surface (name-collision and
//! target-rule checks the frontend cannot see). Split out of `entries/mod.rs`
//! to stay under this repo's per-file line ceiling.

use super::{
    arg_identifiers, companion_name, has_source, local_name, module_entries, op_local_name,
};
use crate::codegen::output::TargetKind;
use crate::ir::{
    CallArg, Module, ShapeKind, Source, Tref, WireBinding, WireCall, WireCallArg, WireValue,
};

/// The requested targets that cannot emit an extern-call field source, by
/// output name. `None` means every target in this run can, so a call-
/// touching field passes. A run naming no target vouches for nothing, so it
/// counts as unsupported rather than vacuously supported.
fn ext_calls_unsupported_by(targets: &[TargetKind]) -> Option<String> {
    unsupported_by(targets, TargetKind::emits_ext_calls)
}

/// The requested targets that cannot emit a foreign opaque-handle type on
/// the entry surface, by output name. Separate from
/// [`ext_calls_unsupported_by`]: a target can land call emission before it
/// also spells the handle's own field type, and a handle-typed field must
/// stay rejected on that target until it does.
fn ext_handle_types_unsupported_by(targets: &[TargetKind]) -> Option<String> {
    unsupported_by(targets, TargetKind::emits_ext_handle_types)
}

/// The requested targets that cannot emit an operation's own
/// `impl .field.method(args)` body, by output name.
fn ext_handle_calls_unsupported_by(targets: &[TargetKind]) -> Option<String> {
    unsupported_by(targets, TargetKind::emits_ext_handle_calls)
}

fn unsupported_by(targets: &[TargetKind], capable: fn(TargetKind) -> bool) -> Option<String> {
    if targets.is_empty() {
        return Some("no target requested".to_string());
    }
    let names: Vec<&str> = targets
        .iter()
        .filter(|t| !capable(**t))
        .map(|t| t.dir())
        .collect();
    (!names.is_empty()).then(|| names.join(", "))
}

/// The sibling field a `CallArg::Ref` path names, recursively unwrapping the
/// argument shapes a foreign call's own arguments can nest in (a struct
/// literal's fields, a list's items): every `Ref` anywhere in a call's own
/// argument tree ultimately reads a sibling field, so all of them share this
/// one check.
pub(super) fn ref_paths(args: &[CallArg], out: &mut Vec<Vec<String>>) {
    for a in args {
        match a {
            CallArg::Ref(path) => out.push(path.clone()),
            CallArg::List(items) => ref_paths(items, out),
            CallArg::Ctor(ctor) => {
                let fields: Vec<CallArg> = ctor.fields.values().cloned().collect();
                ref_paths(&fields, out);
            }
            CallArg::SymbolCall(sc) => ref_paths(&sc.args, out),
            CallArg::ParamAs { .. } | CallArg::Foreign(_) => {}
            CallArg::Param(_) | CallArg::Lit(_) | CallArg::Call(_) | CallArg::TypeRef(_) => {}
        }
    }
}

/// Every top-level `WireCall` directly on a `@header`/`@body` value (never
/// `@query`: unlike headers/body, the URL is already finalized by the time
/// a call could patch it in, so a call there has nowhere to run -- the
/// frontend only ever binds one to header/body in the first place, this is
/// belt-and-suspenders for hand-fed IR). `uri`/`endpoint` carry no call
/// form at all (rejected upstream, at the frontend's protocol-trait
/// position check).
fn top_level_wire_calls(wire: &WireBinding) -> Vec<&WireCall> {
    let mut out = Vec::new();
    for (_, v) in &wire.request_headers {
        if let WireValue::Call(c) = v {
            out.push(c);
        }
    }
    if let Some(WireValue::Call(c)) = &wire.body {
        out.push(c);
    }
    out
}

/// A `WireValue` reachable anywhere off the binding that carries an extern
/// call in a shape no emitter supports yet: nested inside a `@body` ctor
/// field's value (every target's fallible-call handling needs its own
/// statement, which an object-literal field's expression position has no
/// room for), or anywhere in `@query` (the query string is already
/// finalized before a call could read the assembled request, so `.request`
/// has nothing to read there, and a call that reads no `.request` has no
/// reason to run this late instead of in the field it was declared on).
fn unsupported_nested_wire_call(wire: &WireBinding) -> Option<&'static str> {
    fn object_has_call(v: &WireValue) -> bool {
        match v {
            WireValue::Call(_) => true,
            WireValue::Object(fields) => fields.iter().any(|(_, v)| object_has_call(v)),
            WireValue::Lit(_)
            | WireValue::Field(_)
            | WireValue::Param(_)
            | WireValue::Template(_) => false,
        }
    }
    if wire
        .query
        .iter()
        .any(|(_, v)| matches!(v, WireValue::Call(_)) || object_has_call(v))
    {
        return Some("@query");
    }
    if let Some(WireValue::Object(fields)) = &wire.body {
        if fields.iter().any(|(_, v)| object_has_call(v)) {
            return Some("a @body ctor field");
        }
    }
    None
}

/// A struct-literal mapper as a wire-position call's own argument (e.g.
/// `ns.fn(opts { region: .r })`) is not yet supported by any target's
/// emitter: unlike an entry field's own `= ns.fn(args)` construction call,
/// no emitter here renders a foreign struct literal in this position yet.
fn has_ctor_call_arg(call: &WireCall) -> bool {
    call.args.iter().any(|a| matches!(a, WireCallArg::Ctor(_)))
}

/// A @header/@body-position extern call must resolve to a declared
/// `extern` that carries a binding for every target being generated, the
/// same rule [`call_resolves`] enforces for an entry field's own call
/// source. Without this an emitter would meet "no such ext block" as a
/// pipeline defect -- a panic -- instead of an authoring error.
pub(super) fn wire_call_resolves(
    module: &Module,
    call: &WireCall,
    targets: &[TargetKind],
) -> Result<(), String> {
    let Some(lib) = module.ext_libs.iter().find(|l| l.name == call.ns) else {
        return Err(format!(
            "{}.{}(..): no ext block named {} is declared in this module",
            call.ns, call.fn_name, call.ns
        ));
    };
    let Some(decl) = lib.externs.iter().find(|e| e.name == call.fn_name) else {
        return Err(format!(
            "{}.{}(..): ext {} declares no extern named {}",
            call.ns, call.fn_name, call.ns, call.fn_name
        ));
    };
    for target in targets {
        let Some(lang) = decl
            .langs
            .iter()
            .find(|l| target.binding_langs().contains(&l.lang.as_str()))
        else {
            return Err(format!(
                "{}.{}(..): extern {} declares no {} block; {} codegen has nothing to emit",
                call.ns,
                call.fn_name,
                call.fn_name,
                target.binding_langs()[0],
                target.dir()
            ));
        };
        super::validate_calls::foreign_positions_bind(
            &format!("{}.{}(..)", call.ns, call.fn_name),
            *target,
            lang,
        )?;
        super::validate_calls::class_reference_in_wire_position(
            &format!("{}.{}(..)", call.ns, call.fn_name),
            *target,
            lang,
        )?;
    }
    Ok(())
}

/// The construction-surface names the generated Settings reserves for its
/// transport slots, per binding language key. An entry field with one of
/// these canonical names would collide with the slot member.
const RESERVED_SLOT_FIELDS: [&str; 4] = ["headers", "transport", "http_client", "fetch"];

/// Canonical names an `@arg` field cannot take: they become bare constructor
/// parameters, and these are the locals and parameters the generated
/// constructors already declare (Go's Settings/carrier/values plumbing, the
/// TypeScript config object, both targets' scratch variables).
const RESERVED_ARG_NAMES: [&str; 18] = [
    "s",
    "w",
    "v",
    "n",
    "ok",
    "raw",
    "err",
    "ms",
    "opt",
    "opts",
    "config",
    "values",
    "violations",
    "runtime",
    "probe",
    "dec",
    "decoded",
    "composed",
];

/// Language keywords an `@arg` field cannot take either: a single-word
/// canonical name passes through the camel casing unchanged, so a Go or
/// TypeScript keyword would land verbatim as a parameter name.
const RESERVED_ARG_KEYWORDS: [&str; 43] = [
    "break",
    "case",
    "catch",
    "chan",
    "class",
    "const",
    "continue",
    "default",
    "defer",
    "delete",
    "do",
    "else",
    "enum",
    "fallthrough",
    "finally",
    "for",
    "func",
    "function",
    "go",
    "goto",
    "if",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "map",
    "new",
    "package",
    "range",
    "return",
    "select",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "type",
    "typeof",
    "var",
    "void",
];

/// The Pascal spelling of a canonical name, for collision checks against the
/// generated type surface.
fn pascal_ident(name: &str) -> String {
    crate::codegen::casing::transform(
        name,
        crate::codegen::symbol::SymbolKind::Type,
        &crate::codegen::casing::CasingConfig::new(crate::codegen::casing::CaseStyle::Pascal),
        None,
    )
}

/// Whether a `Tref::Ref` id names an opaque handle declared in one of the
/// module's own `ext` blocks (its id is `"{ext_name}#{type_name}"`, the same
/// `#`-qualified shape id every other reference uses), rather than an
/// ordinary shape in `module.shapes`.
pub(crate) fn is_foreign_ref(module: &crate::ir::Module, id: &str) -> bool {
    id.split_once('#').is_some_and(|(lib, ty)| {
        module
            .ext_libs
            .iter()
            .any(|l| l.name == lib && l.types.iter().any(|t| t.name == ty))
    })
}

/// The `Tref::Ref` id a field's target names, if it is a shape/handle
/// reference (as opposed to a primitive, list, or map).
pub(super) fn ref_id(t: &Tref) -> Option<&str> {
    match t {
        Tref::Ref { id, .. } => Some(id.as_str()),
        _ => None,
    }
}

/// Generation-time validation of the entry surface: the cases the frontend
/// cannot see (they are target rules) and that would otherwise produce
/// uncompilable or silently wrong output. Returns the first offense.
///
/// This also covers the one loose-operation rule: entries are the only
/// supported HTTP client surface, so a loose (non-entry) operation carrying a
/// `wire` binding is rejected here, the same way an entry `@http` op with no
/// endpoint is below. The frontend still accepts a loose `@http` op (nothing
/// about the language changes), so this is a generation-time gap the same as
/// the endpoint check, not a checker rule.
///
/// `targets` is every target this generation call produces. A field touching
/// an `ext` block is rejected unless all of them can emit the construct it
/// touches: a green pass for one target says nothing about another in the
/// same call, and the target that cannot spell the construct would panic or
/// write uncompilable output. Each target answers for itself through
/// [`TargetKind::emits_ext_calls`], [`TargetKind::emits_ext_handle_types`],
/// and [`TargetKind::emits_ext_handle_calls`], so landing the next half of a
/// target's emission touches one of those methods and not this gate.
pub fn validate_entries(model: &crate::ir::Model, targets: &[TargetKind]) -> Result<(), String> {
    let ext_calls_unsupported = ext_calls_unsupported_by(targets);
    let ext_handle_types_unsupported = ext_handle_types_unsupported_by(targets);
    let ext_handle_calls_unsupported = ext_handle_calls_unsupported_by(targets);
    for module in &model.modules {
        for op in &module.operations {
            if let ShapeKind::Operation { wire: Some(_), .. } = &op.kind {
                return Err(format!(
                    "module {}: operation {} carries an @http binding outside an entry; \
                     entries are the only supported HTTP client surface",
                    module.name,
                    local_name(&op.id)
                ));
            }
        }
        super::validate_calls::spelling_references_resolve(module)
            .map_err(|e| format!("module {}: {e}", module.name))?;
        let entries = module_entries(module);
        if entries.is_empty() {
            continue;
        }
        for entry in &entries {
            // The frontend requires an entry @http operation to name its
            // endpoint, but the IR is also accepted straight from a file or
            // stdin, so the gap surfaces here as a clean generation error
            // instead of a panic inside an emitter.
            for op in entry.operations {
                if let ShapeKind::Operation {
                    wire: Some(wire), ..
                } = &op.kind
                {
                    if let Some(position) = unsupported_nested_wire_call(wire) {
                        return Err(format!(
                            "module {}: entry {} operation {} carries an extern call inside {}; no target's emitter supports a call there yet",
                            module.name,
                            entry.name,
                            local_name(&op.id),
                            position
                        ));
                    }
                    for call in top_level_wire_calls(wire) {
                        if has_ctor_call_arg(call) {
                            return Err(format!(
                                "module {}: entry {} operation {} calls {}.{}(..) with a struct-literal argument; no target's emitter supports that in a @header/@body call yet",
                                module.name,
                                entry.name,
                                local_name(&op.id),
                                call.ns,
                                call.fn_name
                            ));
                        }
                        wire_call_resolves(module, call, targets).map_err(|e| {
                            format!(
                                "module {}: entry {} operation {} {}",
                                module.name,
                                entry.name,
                                local_name(&op.id),
                                e
                            )
                        })?;
                    }
                    if wire.endpoint.is_none() {
                        return Err(format!(
                            "module {}: entry {} operation {} carries an @http binding with no endpoint; an entry operation's @http must name its endpoint",
                            module.name,
                            entry.name,
                            local_name(&op.id)
                        ));
                    }
                }
                if let ShapeKind::Operation {
                    impl_call: Some(call),
                    ..
                } = &op.kind
                {
                    if let Some(unsupported) = &ext_handle_calls_unsupported {
                        return Err(format!(
                            "module {}: entry {} operation {} implements itself as a call into .{}.{}(..) (an extern handle method); {} cannot emit that call yet",
                            module.name,
                            entry.name,
                            local_name(&op.id),
                            call.recv.join("."),
                            call.method,
                            unsupported
                        ));
                    }
                    // Resolvable for the targets that will emit it: the gate
                    // above says the target can emit such calls, not that
                    // this method carries a binding for it.
                    super::validate_calls::op_impl_call_resolves(
                        module,
                        &entry.declared(),
                        local_name(&op.id),
                        call,
                        targets,
                    )
                    .map_err(|e| format!("module {}: entry {} {}", module.name, entry.name, e))?;
                }
            }
            super::validate_ownership::forwarded_handle_has_one_reader(module, entry)
                .map_err(|e| format!("module {}: entry {} {}", module.name, entry.name, e))?;
            let declared = entry.declared();
            for field in declared.iter().copied() {
                if RESERVED_SLOT_FIELDS.contains(&field.name.as_str()) {
                    return Err(format!(
                        "module {}: entry {} field {} collides with the generated Settings transport slot of the same name; rename the field",
                        module.name, entry.name, field.name
                    ));
                }
                if has_source(field, |s| matches!(s, Source::Arg)) {
                    // The generated @arg parameter is the canonical name cased, or
                    // its @rename(lang) override verbatim: every spelling the
                    // parameter can take must clear the reserved lists, not just
                    // the canonical one.
                    for candidate in arg_identifiers(field) {
                        if RESERVED_ARG_NAMES.contains(&candidate.as_str()) {
                            return Err(format!(
                                "module {}: entry {} field {} is an @arg but its generated parameter {} is a local the generated constructor already declares; rename the field or its @rename",
                                module.name, entry.name, field.name, candidate
                            ));
                        }
                        if RESERVED_ARG_KEYWORDS.contains(&candidate.as_str()) {
                            return Err(format!(
                                "module {}: entry {} field {} is an @arg but its generated parameter {} is a keyword in a target language; rename the field or its @rename",
                                module.name, entry.name, field.name, candidate
                            ));
                        }
                    }
                }
                // The generated resolution derives a `<field>_why` reason
                // variable and a `<field>_set` flag per field; a sibling
                // field spelling either name collides with them.
                for suffix in ["_why", "_set"] {
                    let derived = format!("{}{suffix}", field.name);
                    if declared.iter().any(|other| other.name == derived) {
                        return Err(format!(
                            "module {}: entry {} declares both {} and {}; the resolution derives a variable named {} for the former, rename one",
                            module.name, entry.name, field.name, derived, derived
                        ));
                    }
                }
                // A construction field resolves within its module: the
                // resolution idiom (config vs structured vs scalar) and the
                // generated type surface both need the referenced shape. An
                // injectable-handle field may instead
                // reference an opaque type declared in the module's own
                // `ext` block: that id lives in `ext_libs`, not
                // `module.shapes`, but is same-module all the same.
                let foreign_target =
                    ref_id(&field.target).is_some_and(|id| is_foreign_ref(module, id));
                if let Tref::Ref { id, .. } = &field.target {
                    if !foreign_target && !module.shapes.iter().any(|shape| shape.id == *id) {
                        return Err(format!(
                            "module {}: entry {} field {} references {}, a shape outside this module; construction fields resolve within their module, move the shape or the entry",
                            module.name, entry.name, field.name, id
                        ));
                    }
                }
                // A field (or config member) touching an `ext` block
                // reaches an emitter that may have no spelling for
                // it: building the resolution plan would panic (a call
                // source) or silently write uncompilable output (a foreign
                // handle field, whose type that target cannot name). Reject
                // it here, in one gate per capability, whenever any
                // requested target has not landed that half of the emission
                // yet -- a target can emit the call before it can also spell
                // the handle's own field type, so the two reject
                // independently.
                if let Some(unsupported) = &ext_calls_unsupported {
                    if field.call.is_some() {
                        return Err(format!(
                            "module {}: entry {} field {} touches an ext block (an extern call); {} cannot emit extern calls yet",
                            module.name, entry.name, field.name, unsupported
                        ));
                    }
                    if let Tref::Ref { id, .. } = &field.target {
                        if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                            if let ShapeKind::Config { fields } = &shape.kind {
                                if let Some(member) = fields.iter().find(|m| m.call.is_some()) {
                                    return Err(format!(
                                        "module {}: entry {} field {} member {} touches an ext block (an extern call); {} cannot emit extern calls yet",
                                        module.name, entry.name, field.name, member.name, unsupported
                                    ));
                                }
                            }
                        }
                    }
                }
                if let Some(unsupported) = &ext_handle_types_unsupported {
                    if foreign_target {
                        return Err(format!(
                            "module {}: entry {} field {} touches an ext block (a foreign handle type); {} cannot emit foreign handle types yet",
                            module.name, entry.name, field.name, unsupported
                        ));
                    }
                    if let Tref::Ref { id, .. } = &field.target {
                        if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                            if let ShapeKind::Config { fields } = &shape.kind {
                                if let Some(member) = fields.iter().find(|m| {
                                    ref_id(&m.target).is_some_and(|id| is_foreign_ref(module, id))
                                }) {
                                    return Err(format!(
                                        "module {}: entry {} field {} member {} touches an ext block (a foreign handle type); {} cannot emit foreign handle types yet",
                                        module.name, entry.name, field.name, member.name, unsupported
                                    ));
                                }
                            }
                        }
                    }
                }
                // Resolvable for the targets that will emit it. Checked even
                // when the gate above let the field through, since that gate
                // says the target can emit calls, not that this call names a
                // declared extern with a binding for it.
                super::validate_calls::call_resolves(module, field, targets)
                    .map_err(|e| format!("module {}: entry {} {}", module.name, entry.name, e))?;
                if let Some(unsupported) = &ext_handle_calls_unsupported {
                    if let Some(call) = &field.handle_call {
                        return Err(format!(
                            "module {}: entry {} field {} is sourced from .{}.{}(..) (an extern handle method); {} cannot emit that call yet",
                            module.name,
                            entry.name,
                            field.name,
                            call.recv.join("."),
                            call.method,
                            unsupported
                        ));
                    }
                }
                super::validate_calls::handle_call_resolves(module, &declared, field, targets)
                    .map_err(|e| format!("module {}: entry {} {}", module.name, entry.name, e))?;
                // A config has no handle of its own to call (a foreign handle
                // only lives on an entry field), so a member sourced from
                // one has no receiver in scope; the frontend rejects it and
                // a hand-fed IR is rejected here rather than in a member
                // emitter that never learned the form.
                if let Tref::Ref { id, .. } = &field.target {
                    if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                        if let ShapeKind::Config { fields } = &shape.kind {
                            if let Some(member) = fields.iter().find(|m| m.handle_call.is_some()) {
                                return Err(format!(
                                    "module {}: entry {} field {} member {} is sourced from a handle method call; a config member has no handle in scope, only an entry field does",
                                    module.name, entry.name, field.name, member.name
                                ));
                            }
                        }
                    }
                }
                super::validate_calls::injected_handle_forwarded_to_another_call(
                    module, &declared, field,
                )
                .map_err(|e| format!("module {}: entry {} {}", module.name, entry.name, e))?;
            }
        }
        // A module with loose operations emits the `Client` interface; an
        // entry named `client` would emit a same-named concrete type next to
        // it.
        if !module.operations.is_empty() && entries.iter().any(|e| e.name == "client") {
            return Err(format!(
                "module {}: the entry client collides with the Client interface the module's loose operations emit; rename the entry or move the loose operations into it",
                module.name
            ));
        }
        // An entry named after another entry's generated companion (its
        // Settings/Config/Option/API types) would emit two same-named types.
        let mut companions: Vec<(String, &str)> = Vec::new();
        let multi = entries.len() > 1;
        for entry in &entries {
            companions.push((companion_name(entry.name, "settings", multi), entry.name));
            companions.push((format!("{}_config", entry.name), entry.name));
            companions.push((format!("{}_option", entry.name), entry.name));
            companions.push((format!("{}_api", entry.name), entry.name));
        }
        for entry in &entries {
            if let Some((companion, owner)) = companions.iter().find(|(c, _)| *c == entry.name) {
                return Err(format!(
                    "module {}: entry {} collides with the {} companion generated for entry {}; rename the entry",
                    module.name, entry.name, companion, owner
                ));
            }
        }
        // A declared shape spelling a generated type (an entry's client type
        // or one of its companions) would emit two same-named types.
        let mut generated: Vec<(String, String)> = Vec::new();
        for entry in &entries {
            generated.push((
                pascal_ident(entry.name),
                format!("client type of entry {}", entry.name),
            ));
        }
        for (companion, owner) in &companions {
            generated.push((
                pascal_ident(companion),
                format!("{companion} companion of entry {owner}"),
            ));
        }
        for shape in &module.shapes {
            if matches!(shape.kind, ShapeKind::Entry { .. }) {
                continue;
            }
            // Compared as the emitted type identifier: a shape is written in
            // its canonical spelling (`settings`) and cased by the codegen,
            // so `settings` next to an entry collides with the generated
            // `Settings` exactly as a shape already spelled `Settings` does.
            let ident = pascal_ident(local_name(&shape.id));
            if let Some((_, what)) = generated.iter().find(|(g, _)| *g == ident) {
                return Err(format!(
                    "module {}: shape {} collides with the {}; rename the shape",
                    module.name, ident, what
                ));
            }
        }
        // The Go constructor is `New` (single entry) or `New<Entry>` (multi):
        // an entry type spelling the same identifier cannot share the package.
        if !multi && entries.iter().any(|e| e.name == "new") {
            return Err(format!(
                "module {}: the entry new collides with the New constructor it generates; rename the entry",
                module.name
            ));
        }
        if multi {
            for entry in &entries {
                if let Some(other) = entries
                    .iter()
                    .find(|o| entry.name == format!("new_{}", o.name))
                {
                    return Err(format!(
                        "module {}: entry {} collides with the New{} constructor generated for entry {}; rename one",
                        module.name,
                        entry.name,
                        pascal_ident(other.name),
                        other.name
                    ));
                }
            }
        }
        // A loose op and an entry op sharing a local name would emit two
        // descriptors/discriminators under the same generated identifier.
        for entry in &entries {
            for op in entry.operations {
                let local = op_local_name(&op.id);
                if module
                    .operations
                    .iter()
                    .any(|loose| local_name(&loose.id) == local)
                {
                    return Err(format!(
                        "module {}: operation {} is declared both loose and in entry {}; the generated companions would collide, rename one",
                        module.name, local, entry.name
                    ));
                }
            }
        }
    }
    Ok(())
}

//! The resolution DAG: ordering an entry's fields so every field a step reads
//! is resolved before it. The edges are sibling references (`@env(.ref)` names,
//! `@format` placeholders, `match` subjects/arms, `@bind` sources, extern-call
//! argument refs), plus the reads a composed config makes through its own
//! members.

use std::collections::HashSet;

use crate::ir::{
    ArmValue, Bind, CallArg, EntryField, EnvName, Module, ShapeKind, Source, TemplatePart, Tref,
};

/// The sibling-field heads a `= ns.fn(args)` call reads: every `CallArg::Ref`
/// head, recursing into a ctor field's values and a list's items. A `Param`/
/// `Lit` reads nothing; a nested call (an arg that is itself a call) recurses
/// the same way.
fn call_arg_heads<'a>(arg: &'a CallArg, out: &mut Vec<&'a str>) {
    match arg {
        CallArg::Ref(path) => out.extend(path.first().map(String::as_str)),
        CallArg::Ctor(ctor) => {
            for v in ctor.fields.values() {
                call_arg_heads(v, out);
            }
        }
        CallArg::List(items) => {
            for item in items {
                call_arg_heads(item, out);
            }
        }
        CallArg::Call(call) => {
            for a in &call.args {
                call_arg_heads(a, out);
            }
        }
        CallArg::Param(_) | CallArg::Lit(_) => {}
    }
}

/// The sibling entry fields one field's resolution reads directly (its own
/// `@env(.ref)`, `@format`, `match`, `@bind`, and extern-call argument heads),
/// without descending into a composed config's members.
fn own_dep_heads(field: &EntryField) -> Vec<&str> {
    fn head(p: &[String]) -> Option<&str> {
        p.first().map(String::as_str)
    }
    let mut deps: Vec<&str> = Vec::new();
    for source in &field.sources {
        if let Source::Env(EnvName::Field(fr)) = source {
            deps.extend(head(&fr.field));
        }
    }
    for part in field.format.iter().flatten() {
        if let TemplatePart::Field(p) = part {
            deps.extend(head(p));
        }
    }
    if let Some(select) = &field.select {
        deps.extend(head(&select.subject));
        for arm in &select.arms {
            match &arm.value {
                ArmValue::Field(p) => deps.extend(head(p)),
                ArmValue::Sources(sources) => {
                    for source in sources {
                        if let Source::Env(EnvName::Field(fr)) = source {
                            deps.extend(head(&fr.field));
                        }
                    }
                }
                ArmValue::Lit(_) => {}
                // Names the subject itself, already added above.
                ArmValue::Subject => {}
            }
        }
    }
    if let Some(call) = &field.call {
        for arg in &call.args {
            call_arg_heads(arg, &mut deps);
        }
    }
    for Bind { source, .. } in &field.binds {
        deps.extend(head(source));
    }
    deps
}

/// The sibling fields a field's resolution reads, i.e. its dependency edges in
/// the resolution DAG: `@env(.ref)` variable names, `@format` placeholders,
/// the match subject and its arms' references, and `@bind` sources. Paths into
/// a composed field depend on its head field only.
///
/// A composed config also reads whatever its own members read: each member's
/// `@env(.ref)`/`@format`/`match` resolves against the same sibling scope, so
/// the config must be ordered after every entry field those members reach.
fn dependencies<'a>(field: &'a EntryField, module: &'a Module) -> Vec<&'a str> {
    let mut deps = own_dep_heads(field);
    if let Tref::Ref { id, .. } = &field.target {
        if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
            if let ShapeKind::Config { fields } = &shape.kind {
                for member in fields {
                    deps.extend(own_dep_heads(member));
                }
            }
        }
    }
    deps
}

/// Order fields so every dependency resolves before its dependents (Kahn over
/// the sibling-reference edges), keeping declaration order among ready fields.
/// The frontend already rejected cycles; if malformed input still has one, the
/// remaining fields append in declaration order rather than dropping.
pub(super) fn resolution_order<'a>(
    fields: &'a [EntryField],
    module: &'a Module,
) -> Vec<&'a EntryField> {
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut out: Vec<&EntryField> = Vec::new();
    while out.len() < fields.len() {
        let mut progressed = false;
        for field in fields {
            if placed.contains(field.name.as_str()) {
                continue;
            }
            let ready = dependencies(field, module)
                .into_iter()
                .filter(|d| names.contains(d))
                .all(|d| placed.contains(d));
            if ready {
                placed.insert(field.name.as_str());
                out.push(field);
                progressed = true;
            }
        }
        if !progressed {
            for field in fields {
                if placed.insert(field.name.as_str()) {
                    out.push(field);
                }
            }
        }
    }
    out
}

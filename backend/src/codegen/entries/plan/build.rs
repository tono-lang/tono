//! The builders that shape the resolution [`Stmt`] tree from an [`EntryModel`].
//!
//! Every function here is language-neutral: it decides the control flow (which
//! source wins, how a config composes, how a match lowers, which consumed
//! chains need a post-construction check) and asks the [`Emitter`] only for the
//! per-target leaf spellings. The tree it returns is walked by
//! [`super::render`].

use super::super::{EntryModel, FieldShape};
use super::{arm_sources, format_absent_deps, string_like, Emitter, Leaf, Stmt};
use crate::ir::{
    ArmValue, EntryField, EnvName, Module, Prim, Select, Shape, ShapeKind, Source, Tref,
};

fn has_arg(field: &EntryField) -> bool {
    field.sources.iter().any(|s| matches!(s, Source::Arg))
}

/// The logical error-var name tracking a composed member's resolution,
/// distinct from any entry field's own error variable.
fn member_err_name(field: &EntryField, member: &EntryField) -> String {
    format!("{}_{}", field.name, member.name)
}

/// Whether a composed config member needs a hoisted error variable so a
/// consumed non-string member can be required at construction instead of being
/// frozen at its silent zero (a legitimately resolved `0`/`false` is
/// indistinguishable from absence by value alone). Scoped to the plain
/// non-guaranteed numeric/bool chain that a descriptor consumes: bound members
/// and derivations keep their zero, out of this first cut. Returns the logical
/// error-var name when tracking is needed.
fn member_needs_err(field: &EntryField, member: &EntryField, entry: &EntryModel) -> Option<String> {
    if member.select.is_some() || member.format.is_some() || member.call.is_some() {
        return None;
    }
    if field.binds.iter().any(|b| b.field == member.name) {
        return None;
    }
    if member
        .sources
        .iter()
        .any(|s| matches!(s, Source::Arg | Source::Default(_)))
    {
        return None;
    }
    let leaf = &member.target;
    if !is_numeric(leaf) && !matches!(leaf, Tref::Prim(Prim::Bool)) {
        return None;
    }
    let consumed = entry
        .consumed_field_paths()
        .iter()
        .any(|p| p.len() == 2 && p[0] == field.name && p[1] == member.name);
    consumed.then(|| member_err_name(field, member))
}

/// Build the resolution plan for one entry field, dispatching on its shape.
pub fn build_field<'a>(
    field: &'a EntryField,
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    e: &mut dyn Emitter,
) -> Stmt {
    match entry.field_shape(field, module) {
        FieldShape::Config(shape) => build_config(field, shape, entry, e),
        FieldShape::Structured(shape) => Stmt::Leaf(Leaf(e.structured_body(field, shape))),
        FieldShape::Json => Stmt::Leaf(Leaf(e.json_body(field))),
        FieldShape::Scalar => build_scalar(field, entry, e),
        FieldShape::Call => {
            let dest = e.dest(&field.name);
            build_call_field(field, e, &dest)
        }
    }
}

/// A `= ns.fn(args)` extern-call source: a plain call is an
/// unconditional assignment, always attempted, like a config compose or a
/// `@default`. A call alongside `@with` tries the injected
/// value first and falls back to the call, the same two-route shape
/// [`chain_sequential`]'s `Source::With` arm already gives a scalar chain.
/// The call's own spelling is a per-target leaf, deferred to codegen. `dest`
/// is the caller's destination (a top-level field or a config member path).
fn build_call_field(field: &EntryField, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let Some(call) = &field.call else {
        return Stmt::Nop;
    };
    let assign = Stmt::Leaf(Leaf(e.call_assign(field, call, dest)));
    if !field.sources.iter().any(|s| matches!(s, Source::With)) {
        return assign;
    }
    // The injected value wins when present; the call is the construction
    // fallback otherwise. No error tracking here (contrast a scalar chain's
    // `Source::With` step, [`chain_sequential`]): absence just means "run
    // the fallback call", never a deferred failure to report, so this reads
    // as a plain `if`/`else` rather than the error-var cascade.
    Stmt::If {
        arms: vec![(
            e.with_present_cond(field),
            Stmt::Leaf(e.with_assign(field, dest)),
        )],
        otherwise: Some(Box::new(assign)),
    }
}

fn is_numeric(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(
            Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
                | Prim::Float
        )
    )
}

/// How a resolved value must be probed for presence before a declared
/// validation guard runs over it (the guard must not reject a legitimately
/// resolved zero/empty). A guaranteed or composed field is always present; a
/// bool has no absent-vs-zero distinction so it needs no probe either.
pub enum Presence {
    Always,
    String,
    Bytes,
    Numeric,
}

/// Classify a field's presence probe for the declared-validation guards. The
/// dispatch is shared; each target spells the resulting probe in its own syntax.
pub fn presence_kind(field: &EntryField, entry: &EntryModel, module: &Module) -> Presence {
    if entry.is_guaranteed(field)
        || matches!(
            entry.field_shape(field, module),
            FieldShape::Config(_) | FieldShape::Call
        )
    {
        Presence::Always
    } else if string_like(&field.target) {
        Presence::String
    } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
        Presence::Bytes
    } else if is_numeric(&field.target) {
        Presence::Numeric
    } else {
        Presence::Always
    }
}

/// The consumed-chain requires: for every consumed field path, whether a
/// post-construction presence check is needed and of which kind. The selection
/// (skip guaranteed scalars, bools, and non-string decoded members) is shared;
/// each target spells the comparison and the error. Runs after `client_init`, so
/// every check reads the resolved value and the error-value only decorates it.
pub fn build_requires(entry: &EntryModel, module: &Module, e: &mut dyn Emitter) -> Stmt {
    let mut out: Vec<Stmt> = Vec::new();
    for path in entry.consumed_field_paths() {
        let Some(head) = path.first() else {
            continue;
        };
        let Some(field) = entry.fields.iter().find(|f| f.name == *head) else {
            continue;
        };
        let shape = entry.field_shape(field, module);
        if path.len() > 1 && matches!(shape, FieldShape::Config(_) | FieldShape::Structured(_)) {
            let leaf = entry.path_type(&path, module);
            if string_like(&leaf) {
                out.push(Stmt::Leaf(Leaf(e.require_member(
                    head,
                    &path[1],
                    &leaf,
                    &path.join("."),
                ))));
                continue;
            }
            // A consumed numeric/bool config member carries a hoisted error var
            // (a resolved zero is not absence), so its require reads that var,
            // not the value. Members without one keep their zero, as before.
            if let FieldShape::Config(cfg) = shape {
                if let ShapeKind::Config { fields } = &cfg.kind {
                    if let Some(member) = fields.iter().find(|m| m.name == path[1]) {
                        if let Some(err) = member_needs_err(field, member, entry) {
                            out.push(Stmt::Leaf(Leaf(
                                e.require_member_deferred(&path.join("."), &err),
                            )));
                        }
                    }
                }
            }
            continue;
        }
        if !matches!(shape, FieldShape::Scalar) || entry.is_guaranteed(field) {
            continue;
        }
        let check = if string_like(&field.target) {
            Some(e.require_string(head, &field.target))
        } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
            Some(e.require_bytes(head))
        } else if is_numeric(&field.target) {
            Some(e.require_numeric(head, &field.target))
        } else {
            None
        };
        if let Some(check) = check {
            out.push(Stmt::Leaf(Leaf(check)));
        }
    }
    seq(out)
}

/// A scalar field: an `@arg`, a match, a `@format` derivation, or a plain
/// chain, then the declared `@str::*` pipeline over whatever it produced
/// (`@format` folds the pipeline into its own template).
fn build_scalar(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter) -> Stmt {
    let dest = e.dest(&field.name);
    if field.format.is_some() {
        return build_format(field, entry, e, &dest);
    }
    let head = if has_arg(field) {
        Stmt::Leaf(e.assign_arg(field, &dest))
    } else if field.select.is_some() {
        build_select(field, entry, e, &dest)
    } else {
        build_chain(field, entry, e, &dest)
    };
    seq(vec![
        head,
        opt_leaf(e.transforms_body(field, &dest).map(Leaf)),
    ])
}

/// A `@format` derivation: the assignment is a per-target leaf (template concat,
/// cast, and the `@str::*` pipeline folded in); the deferral guard is shared. A
/// template that reads a deferred head assigns only once every head resolves,
/// carrying the last miss reason otherwise.
fn build_format(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let Some(parts) = field.format.clone() else {
        return Stmt::Nop;
    };
    let assign = Stmt::Leaf(Leaf(e.format_assign(field, dest)));
    let deps = format_absent_deps(entry, &parts);
    if deps.is_empty() {
        return assign;
    }
    let arms = deps
        .iter()
        .map(|dep| {
            (
                e.cond_err_present(dep),
                Stmt::Leaf(e.wrap_from(&field.name, dep)),
            )
        })
        .collect();
    seq(vec![
        Stmt::Leaf(e.err_open(&field.name)),
        Stmt::If {
            arms,
            otherwise: Some(Box::new(assign)),
        },
    ])
}

/// The plain source chain of one field. A guaranteed chain is a per-target
/// leaf (Go and TypeScript spell it with different algorithms). A
/// non-guaranteed one opens an error-value var and tries each source in turn,
/// sharing the sequential-fallback shape.
fn build_chain(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    if entry.is_guaranteed(field) {
        if extraction_blocked(field) {
            Stmt::Leaf(Leaf(e.chain_guaranteed(field, dest)))
        } else {
            Stmt::Leaf(Leaf(e.resolve_fn_call(field, dest)))
        }
    } else {
        let err = field.name.clone();
        seq(vec![
            Stmt::Leaf(e.err_open(&err)),
            chain_sequential(field, e, dest, &err),
        ])
    }
}

/// Whether an `@env` source of this field blocks extraction into a
/// standalone resolver function: a name derived from a sibling field
/// (`EnvName::Field`) reads that sibling's already-resolved value, which a
/// standalone function has no access to without a second parameter; a
/// fallible parse (see [`env_parse_may_fail`]) fails through a "return the
/// error" statement built for the constructor's own `(value, error)`/
/// `Result<Self, _>` return shape, which does not fit a function returning a
/// bare value. Both stay on the inline cascade every target already emits
/// correctly.
fn extraction_blocked(field: &EntryField) -> bool {
    field.sources.iter().any(|s| match s {
        Source::Env(EnvName::Field(_)) => true,
        Source::Env(_) => env_parse_may_fail(&field.target),
        _ => false,
    })
}

/// Whether a target type's `@env` parse (see each target's own `env_parse`)
/// can fail: every target shares the same fallible set (a typed parse with a
/// range or format to reject) and the same infallible catch-all (a plain
/// string-shaped cast, never rejected).
fn env_parse_may_fail(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(
            Prim::Bool
                | Prim::I8
                | Prim::I16
                | Prim::I32
                | Prim::I64
                | Prim::U8
                | Prim::U16
                | Prim::U32
                | Prim::U64
                | Prim::Float
                | Prim::Bytes
                | Prim::Duration
        )
    )
}

/// A non-guaranteed chain: each source is a "still absent?" step. The first
/// runs unconditionally; every later one is guarded by the error-value var
/// still being set, so the run reads as sequential fallbacks carrying the
/// last failure.
fn chain_sequential(field: &EntryField, e: &mut dyn Emitter, dest: &str, err: &str) -> Stmt {
    let mut out: Vec<Stmt> = Vec::new();
    let mut first = true;
    for source in &field.sources {
        let step = match source {
            Source::With => {
                let w = e.err_ident(err);
                Stmt::Leaf(Leaf(e.with_step_body(field, dest, &w)))
            }
            Source::Env(name) => {
                let w = e.err_ident(err);
                Stmt::Leaf(Leaf(e.env_step_body(field, name, dest, &w)))
            }
            Source::Default(v) => seq(vec![
                Stmt::Leaf(e.assign_default(field, v, dest)),
                Stmt::Leaf(e.err_clear(err)),
            ]),
            Source::Arg => continue,
        };
        if step.is_nop() {
            continue;
        }
        if first {
            out.push(step);
            first = false;
        } else {
            out.push(Stmt::If {
                arms: vec![(e.cond_err_present(err), step)],
                otherwise: None,
            });
        }
    }
    seq(out)
}

/// A composed config: a brace scope building `composed`, one member at a time.
/// An entry `@bind` wins over the member's own sources; a non-guaranteed bind
/// falls back to those sources when the bound value is absent.
fn build_config(
    field: &EntryField,
    shape: &Shape,
    entry: &EntryModel,
    e: &mut dyn Emitter,
) -> Stmt {
    let ShapeKind::Config { fields } = &shape.kind else {
        return Stmt::Nop;
    };
    // A consumed non-string member's error variable must outlive the config
    // brace scope so the post-construction require can read it; hoist it above
    // the block that resolves the member.
    let mut hoisted: Vec<Stmt> = Vec::new();
    for member in fields {
        if let Some(err) = member_needs_err(field, member, entry) {
            hoisted.push(Stmt::Leaf(e.err_open(&err)));
        }
    }
    let open = e.config_open(field, shape);
    let mut members: Vec<Stmt> = Vec::new();
    for member in fields {
        let member_dest = e.member_dest(&member.name);
        let err = member_needs_err(field, member, entry);
        let bind = field.binds.iter().find(|b| b.field == member.name);
        members.push(match bind {
            Some(bind) => {
                let head = bind.source.first().cloned().unwrap_or_default();
                let expr = e.bind_expr(&bind.source);
                if entry.field_guaranteed(&head) {
                    Stmt::Leaf(e.member_bind_assign(&member_dest, &expr))
                } else {
                    // The bound value wins when resolved; otherwise the member
                    // falls back to its own sources.
                    Stmt::If {
                        arms: vec![(
                            e.cond_err_absent(&head),
                            Stmt::Leaf(e.member_bind_assign(&member_dest, &expr)),
                        )],
                        otherwise: Some(Box::new(build_member(
                            member,
                            entry,
                            e,
                            &member_dest,
                            err.as_deref(),
                        ))),
                    }
                }
            }
            None => build_member(member, entry, e, &member_dest, err.as_deref()),
        });
    }
    let block = Stmt::Block {
        open,
        body: Box::new(seq(members)),
        close: e.config_close(&e.dest(&field.name)),
    };
    if hoisted.is_empty() {
        block
    } else {
        hoisted.push(block);
        seq(hoisted)
    }
}

/// A config member's own resolution: a match, a `@format` derivation, or its
/// source chain, plus its `@str::*` pipeline (`@format` folds it in). `err` is
/// the hoisted error variable a consumed non-string member tracks (see
/// [`member_needs_err`]); its absence keeps the error-less cascade.
fn build_member(
    member: &EntryField,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
    err: Option<&str>,
) -> Stmt {
    let head = if member.select.is_some() {
        build_member_select(member, entry, e, dest)
    } else if member.format.is_some() {
        build_format(member, entry, e, dest)
    } else if member.call.is_some() {
        build_call_field(member, e, dest)
    } else if let Some(err) = err {
        // A consumed non-string member tracks an error so its absence can be
        // required at construction (a resolved `0`/`false` is not absence). The
        // error var is opened by the caller above the config block; here each
        // source is a still-absent fallback step, as in a scalar chain.
        chain_sequential(&arm_sources(member, &member.sources), e, dest, err)
    } else {
        // A member resolves through the same ordered cascade as a field chain
        // (first present source wins, an optional @default closes it); it carries
        // no error var, so an unresolved optional member keeps its zero value.
        Stmt::Leaf(Leaf(
            e.chain_guaranteed(&arm_sources(member, &member.sources), dest),
        ))
    };
    if member.format.is_some() {
        head
    } else {
        seq(vec![
            head,
            opt_leaf(e.transforms_body(member, dest).map(Leaf)),
        ])
    }
}

/// A scalar `match` lowered to a switch: a deferred subject defers the whole
/// field (it records the reason and skips the switch); a resolved subject picks
/// an arm. A non-guaranteed field opens its error var first and every
/// unmatched value is a failure (a guaranteed field) or a recorded miss (a
/// deferred one).
fn build_select(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let Some(select) = field.select.clone() else {
        return Stmt::Nop;
    };
    let guaranteed = entry.is_guaranteed(field);
    let err = field.name.clone();
    let subject_head = select.subject.first().cloned().unwrap_or_default();
    let subject_expr = e.path_read(&select.subject);
    let switch = build_switch(
        field,
        &select,
        entry,
        e,
        dest,
        &subject_expr,
        guaranteed,
        false,
    );
    let guarded = if entry.field_guaranteed(&subject_head) {
        switch
    } else {
        Stmt::If {
            arms: vec![(
                e.cond_err_present(&subject_head),
                Stmt::Leaf(e.wrap_from(&err, &subject_head)),
            )],
            otherwise: Some(Box::new(switch)),
        }
    };
    if guaranteed {
        guarded
    } else {
        seq(vec![Stmt::Leaf(e.err_open(&err)), guarded])
    }
}

/// A config member `match`: no reason tracking, so a deferred subject or an
/// unmatched value simply leaves the member's zero value.
fn build_member_select(
    member: &EntryField,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
) -> Stmt {
    let Some(select) = member.select.clone() else {
        return Stmt::Nop;
    };
    let subject_head = select.subject.first().cloned().unwrap_or_default();
    let subject_expr = e.path_read(&select.subject);
    let switch = build_switch(member, &select, entry, e, dest, &subject_expr, false, true);
    if entry.field_guaranteed(&subject_head) {
        switch
    } else {
        Stmt::If {
            arms: vec![(e.cond_err_absent(&subject_head), switch)],
            otherwise: None,
        }
    }
}

/// The switch node shared by both selection forms: one arm per declared pattern
/// (a top-level match with no wildcard gains a forced miss arm).
#[allow(clippy::too_many_arguments)]
fn build_switch(
    field: &EntryField,
    select: &Select,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
    subject_expr: &str,
    guaranteed: bool,
    member: bool,
) -> Stmt {
    let mut cases: Vec<(String, Stmt)> = Vec::new();
    let mut default: Option<Box<Stmt>> = None;
    for arm in &select.arms {
        let body = if member {
            build_member_arm(field, &arm.value, entry, e, dest)
        } else {
            build_arm(field, &arm.value, entry, e, dest, guaranteed)
        };
        match &arm.pattern {
            Some(pattern) => cases.push((e.pattern_lit(pattern), body)),
            None => default = Some(Box::new(body)),
        }
    }
    if !member && default.is_none() {
        let head = select.subject.first().cloned().unwrap_or_default();
        default = Some(Box::new(Stmt::Leaf(e.select_miss(
            field,
            &head,
            subject_expr,
            guaranteed,
        ))));
    }
    Stmt::Switch {
        subject: subject_expr.to_string(),
        cases,
        default,
    }
}

/// A scalar match arm: a literal, a sibling read (deferred if that sibling is),
/// or an inline source chain.
fn build_arm(
    field: &EntryField,
    value: &ArmValue,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
    guaranteed: bool,
) -> Stmt {
    match value {
        ArmValue::Lit(v) => Stmt::Leaf(e.assign_default(field, v, dest)),
        ArmValue::Field(path) => {
            let head = path.first().cloned().unwrap_or_default();
            let expr = e.path_read(path);
            if entry.field_guaranteed(&head) {
                Stmt::Leaf(e.assign_expr(dest, &expr))
            } else {
                Stmt::If {
                    arms: vec![(
                        e.cond_err_present(&head),
                        Stmt::Leaf(e.wrap_from(&field.name, &head)),
                    )],
                    otherwise: Some(Box::new(Stmt::Leaf(e.assign_expr(dest, &expr)))),
                }
            }
        }
        ArmValue::Sources(sources) => {
            let stub = arm_sources(field, sources);
            if guaranteed {
                Stmt::Leaf(Leaf(e.chain_guaranteed(&stub, dest)))
            } else {
                // No reset needed before the cascade: this arm's case body runs
                // once per dispatch, and the field's error var (opened once above
                // the whole switch) is still nil/undefined on entry here.
                chain_sequential(&stub, e, dest, &field.name)
            }
        }
    }
}

/// A config-member match arm: like [`build_arm`] but with no reason tracking (a
/// deferred sibling just skips the assignment, leaving the zero value).
fn build_member_arm(
    member: &EntryField,
    value: &ArmValue,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
) -> Stmt {
    match value {
        ArmValue::Lit(v) => Stmt::Leaf(e.assign_default(member, v, dest)),
        ArmValue::Field(path) => {
            let head = path.first().cloned().unwrap_or_default();
            let expr = e.path_read(path);
            if entry.field_guaranteed(&head) {
                Stmt::Leaf(e.assign_expr(dest, &expr))
            } else {
                Stmt::If {
                    arms: vec![(
                        e.cond_err_absent(&head),
                        Stmt::Leaf(e.assign_expr(dest, &expr)),
                    )],
                    otherwise: None,
                }
            }
        }
        ArmValue::Sources(sources) => Stmt::Leaf(Leaf(
            e.chain_guaranteed(&arm_sources(member, sources), dest),
        )),
    }
}

fn seq(stmts: Vec<Stmt>) -> Stmt {
    let kept: Vec<Stmt> = stmts.into_iter().filter(|s| !s.is_nop()).collect();
    match kept.len() {
        0 => Stmt::Nop,
        _ => Stmt::Seq(kept),
    }
}

fn opt_leaf(leaf: Option<Leaf>) -> Stmt {
    leaf.map(Stmt::Leaf).unwrap_or(Stmt::Nop)
}

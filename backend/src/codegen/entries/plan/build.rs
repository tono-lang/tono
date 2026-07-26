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

/// The prereq guard an env step opens when the variable's own name is read from
/// a still-deferred sibling: the env lookup runs only once that head resolves,
/// so the step chains onto this guard's `else`. Empty when the name is a literal
/// or a guaranteed sibling. The decision is shared; the guard line is spelled
/// per target.
pub fn env_name_prereq(name: &EnvName, why: &str, entry: &EntryModel, e: &dyn Emitter) -> String {
    let EnvName::Field(fr) = name else {
        return String::new();
    };
    let Some(head) = fr.field.first() else {
        return String::new();
    };
    if entry.field_guaranteed(head) {
        return String::new();
    }
    e.name_prereq_line(head, why)
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
        || matches!(entry.field_shape(field, module), FieldShape::Config(_))
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
/// every check reads the resolved value and the why-reason only decorates it.
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
            if !string_like(&leaf) {
                continue;
            }
            out.push(Stmt::Leaf(Leaf(e.require_member(
                head,
                &path[1],
                &leaf,
                &path.join("."),
            ))));
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
                e.cond_why_absent(dep),
                Stmt::Leaf(e.assign_reason(&field.name, dep)),
            )
        })
        .collect();
    seq(vec![
        Stmt::Leaf(e.why_open(&field.name, "")),
        Stmt::If {
            arms,
            otherwise: Some(Box::new(assign)),
        },
    ])
}

/// The plain source chain of one field. A guaranteed chain is a per-target
/// leaf (Go and TypeScript spell it with different algorithms). A
/// non-guaranteed one opens a why-var and tries each source in turn, sharing
/// the sequential-fallback shape.
fn build_chain(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    if entry.is_guaranteed(field) {
        Stmt::Leaf(Leaf(e.chain_guaranteed(field, dest)))
    } else {
        let why = field.name.clone();
        seq(vec![
            Stmt::Leaf(e.why_open(&why, "no source")),
            chain_sequential(field, e, dest, &why),
        ])
    }
}

/// A non-guaranteed chain: each source is a "still absent?" step. The first
/// runs unconditionally; every later one is guarded by the why-var still being
/// set, so the run reads as sequential fallbacks carrying the last reason.
fn chain_sequential(field: &EntryField, e: &mut dyn Emitter, dest: &str, why: &str) -> Stmt {
    let mut out: Vec<Stmt> = Vec::new();
    let mut first = true;
    for source in &field.sources {
        let step = match source {
            Source::With => {
                let w = e.why_ident(why);
                Stmt::Leaf(Leaf(e.with_step_body(field, dest, &w)))
            }
            Source::Env(name) => {
                let w = e.why_ident(why);
                Stmt::Leaf(Leaf(e.env_step_body(field, name, dest, &w)))
            }
            Source::Default(v) => seq(vec![
                Stmt::Leaf(e.assign_default(field, v, dest)),
                Stmt::Leaf(e.why_set(why, "")),
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
                arms: vec![(e.cond_why_absent(why), step)],
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
    let open = e.config_open(field, shape);
    let mut members: Vec<Stmt> = Vec::new();
    for member in fields {
        let member_dest = e.member_dest(&member.name);
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
                            e.cond_why_resolved(&head),
                            Stmt::Leaf(e.member_bind_assign(&member_dest, &expr)),
                        )],
                        otherwise: Some(Box::new(build_member(member, entry, e, &member_dest))),
                    }
                }
            }
            None => build_member(member, entry, e, &member_dest),
        });
    }
    Stmt::Block {
        open,
        body: Box::new(seq(members)),
        close: e.config_close(&e.dest(&field.name)),
    }
}

/// A config member's own resolution: a match, a `@format` derivation, or its
/// source chain, plus its `@str::*` pipeline (`@format` folds it in).
fn build_member(member: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let head = if member.select.is_some() {
        build_member_select(member, entry, e, dest)
    } else if member.format.is_some() {
        build_format(member, entry, e, dest)
    } else {
        Stmt::Leaf(Leaf(
            e.member_chain_body(&arm_sources(member, &member.sources), dest),
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
/// an arm. A non-guaranteed field opens its reason first and every unmatched
/// value is a failure (a guaranteed field) or a recorded miss (a deferred one).
fn build_select(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let Some(select) = field.select.clone() else {
        return Stmt::Nop;
    };
    let guaranteed = entry.is_guaranteed(field);
    let why = field.name.clone();
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
                e.cond_why_absent(&subject_head),
                Stmt::Leaf(e.assign_reason(&why, &subject_head)),
            )],
            otherwise: Some(Box::new(switch)),
        }
    };
    if guaranteed {
        guarded
    } else {
        seq(vec![Stmt::Leaf(e.why_open(&why, "")), guarded])
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
            arms: vec![(e.cond_why_resolved(&subject_head), switch)],
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
                        e.cond_why_absent(&head),
                        Stmt::Leaf(e.assign_reason(&field.name, &head)),
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
                seq(vec![
                    Stmt::Leaf(e.why_set(&field.name, "no source")),
                    chain_sequential(&stub, e, dest, &field.name),
                ])
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
                        e.cond_why_resolved(&head),
                        Stmt::Leaf(e.assign_expr(dest, &expr)),
                    )],
                    otherwise: None,
                }
            }
        }
        ArmValue::Sources(sources) => Stmt::Leaf(Leaf(
            e.member_chain_body(&arm_sources(member, sources), dest),
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

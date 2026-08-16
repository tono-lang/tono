//! `match` selection lowering: the switch/optional-switch tree and the arm
//! bodies for both the top-level (reason-tracking) and config-member
//! (zero-value-on-miss) selection forms. Split out of [`super::build`] to
//! keep that file under the line-count ceiling.

use super::super::EntryModel;
use super::build::{chain_sequential, seq};
use super::{arm_sources, Emitter, Leaf, Stmt};
use crate::ir::{ArmValue, EntryField, Select};

/// Whether a match-arm pattern is the mandatory `null` marker (a
/// `{"null": true}` object, not bare JSON `null`, so it survives serde's
/// `Option<Value>` round-trip on the Rust IR mirror without collapsing into
/// the wildcard arm).
fn is_null_pattern(pattern: &Option<serde_json::Value>) -> bool {
    matches!(pattern, Some(serde_json::Value::Object(m)) if m.get("null") == Some(&serde_json::Value::Bool(true)))
}

/// A config member `match`: no reason tracking, so a deferred subject or an
/// unmatched value simply leaves the member's zero value.
pub(super) fn build_member_select(
    member: &EntryField,
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
) -> Stmt {
    let Some(select) = member.select.clone() else {
        return Stmt::Nop;
    };
    let subject_head = select.subject.first().cloned().unwrap_or_default();
    let switch = if let Some(key_path) = select.subject_index.clone() {
        build_optional_switch(member, &select, &key_path, entry, e, dest, false, true)
    } else {
        let subject_expr = e.path_read(&select.subject);
        build_switch(member, &select, entry, e, dest, &subject_expr, false, true)
    };
    if entry.field_guaranteed(&subject_head) {
        switch
    } else {
        Stmt::If {
            arms: vec![(e.cond_err_absent(&subject_head), switch)],
            otherwise: None,
        }
    }
}

/// A map-indexed match subject: indexing a map can always miss, so this binds
/// the lookup once (per-target comma-ok / `Option` / presence idiom, never a
/// target zero value standing in for absence) and branches on it directly
/// rather than folding "present" into an ordinary switch case. The mandatory
/// `null` arm becomes the absent branch; every other declared arm runs, as an
/// ordinary switch over the bound (now non-optional) value, in the present
/// branch, where `._` reads the same binding back out instead of re-indexing.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_optional_switch(
    field: &EntryField,
    select: &Select,
    key_path: &[String],
    entry: &EntryModel,
    e: &mut dyn Emitter,
    dest: &str,
    guaranteed: bool,
    member: bool,
) -> Stmt {
    let key_expr = e.path_read(key_path);
    let bind = e.map_index_bind(&select.subject, &key_expr, &field.name);
    let present_cond = e.map_index_present_cond(&field.name);
    let present_expr = e.map_index_value_expr(&field.name);
    let null_arm = select.arms.iter().find(|a| is_null_pattern(&a.pattern));
    let other_arms: Vec<_> = select
        .arms
        .iter()
        .filter(|a| !is_null_pattern(&a.pattern))
        .cloned()
        .collect();
    let inner = Select {
        subject: select.subject.clone(),
        subject_index: None,
        arms: other_arms,
    };
    let present_body = build_switch(
        field,
        &inner,
        entry,
        e,
        dest,
        &present_expr,
        guaranteed,
        member,
    );
    // The frontend requires exactly one 'null' arm on an optional subject, so
    // this is only ever absent for malformed input the frontend already
    // rejected; leave the destination untouched rather than guessing.
    let absent_body = match null_arm {
        Some(arm) if member => build_member_arm(field, &arm.value, entry, e, dest),
        Some(arm) => build_arm(field, &arm.value, entry, e, dest, guaranteed),
        None => Stmt::Nop,
    };
    seq(vec![
        Stmt::Leaf(bind),
        Stmt::If {
            arms: vec![(present_cond, present_body)],
            otherwise: Some(Box::new(absent_body)),
        },
    ])
}

/// The switch node shared by both selection forms: one arm per declared pattern
/// (a top-level match with no wildcard gains a forced miss arm).
#[allow(clippy::too_many_arguments)]
pub(super) fn build_switch(
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
    // A lone wildcard arm (the only shape left after build_optional_switch
    // strips the 'null' arm from a single-arm '_ => ._' present branch, or a
    // match that was always just a fallback) has nothing to dispatch on: a
    // one-case switch is dead machinery nobody would write by hand, so run
    // the arm directly instead of wrapping it.
    if cases.is_empty() {
        if let Some(body) = default {
            return *body;
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
        // "._": the same path the switch already dispatches on, read back
        // out. The subject is present by construction in every non-null
        // arm (the null arm is handled separately, before this function is
        // reached for the rest), so this follows the same guaranteed/deferred
        // shape as an ordinary field reference to that path.
        ArmValue::Subject => {
            let select = field.select.as_ref();
            if select.and_then(|s| s.subject_index.as_ref()).is_some() {
                // The present branch already bound this value (the switch's
                // own subject is the binding, not a re-index); read it back
                // directly, already narrowed non-optional.
                let expr = e.map_index_value_expr(&field.name);
                Stmt::Leaf(e.assign_expr(dest, &expr))
            } else {
                let path = select.map(|s| s.subject.clone()).unwrap_or_default();
                let head = path.first().cloned().unwrap_or_default();
                let expr = e.path_read(&path);
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
        ArmValue::Subject => {
            let select = member.select.as_ref();
            if select.and_then(|s| s.subject_index.as_ref()).is_some() {
                let expr = e.map_index_value_expr(&member.name);
                Stmt::Leaf(e.assign_expr(dest, &expr))
            } else {
                let path = select.map(|s| s.subject.clone()).unwrap_or_default();
                let head = path.first().cloned().unwrap_or_default();
                let expr = e.path_read(&path);
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
        }
    }
}

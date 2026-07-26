//! Shape-level helpers for the compatibility checker: human-readable
//! spellings for change details, constraint-tightening comparisons, and the
//! reference walk that decides whether a removed shape is still reachable.

use crate::ir::{Constraint, EnumBacking, Shape, ShapeKind, Tref};

/// A compact, human-readable spelling of a type reference for change details.
pub(crate) fn render_tref(t: &Tref) -> String {
    match t {
        Tref::Prim(p) => format!("{p:?}").to_lowercase(),
        Tref::Param(name) => name.clone(),
        Tref::Ref { id, args } if args.is_empty() => id.clone(),
        Tref::Ref { id, args } => {
            let inner: Vec<String> = args.iter().map(render_tref).collect();
            format!("{id}<{}>", inner.join(", "))
        }
        Tref::List(inner) => format!("list<{}>", render_tref(inner)),
        Tref::Map(k, v) => format!("map<{}, {}>", render_tref(k), render_tref(v)),
    }
}

pub(crate) fn render_opt(t: &Option<Tref>) -> String {
    match t {
        Some(t) => render_tref(t),
        None => "none".into(),
    }
}

pub(crate) fn kind_name(kind: &ShapeKind) -> &'static str {
    match kind {
        ShapeKind::Structure { .. } => "structure",
        ShapeKind::Union { .. } => "union",
        ShapeKind::Enum { .. } => "enum",
        ShapeKind::Service { .. } => "service",
        ShapeKind::Operation { .. } => "operation",
        ShapeKind::Entry { .. } => "entry",
        ShapeKind::Config { .. } => "config",
    }
}

pub(crate) fn backing_name(b: &EnumBacking) -> &'static str {
    match b {
        EnumBacking::String => "string",
        EnumBacking::Int => "int",
    }
}

pub(crate) fn constraint_name(c: &Constraint) -> &'static str {
    match c {
        Constraint::Range { .. } => "range",
        Constraint::Length { .. } => "length",
        Constraint::Pattern(_) => "pattern",
        Constraint::MultipleOf(_) => "multipleOf",
    }
}

/// Whether two constraints are the same variant (so they pair up for a tightening
/// comparison rather than reading as an add/remove).
pub(crate) fn same_constraint_kind(a: &Constraint, b: &Constraint) -> bool {
    constraint_name(a) == constraint_name(b)
}

/// Whether `new` rejects values `old` accepted. Bounds narrowing (a raised min, a
/// lowered max, or an inclusive bound made exclusive) tightens; `Pattern` and
/// `MultipleOf` are treated conservatively (any change tightens) since proving a
/// looser regex or modulus dependency-free is not worth it.
pub(crate) fn tightened(old: &Constraint, new: &Constraint) -> bool {
    match (old, new) {
        (
            Constraint::Range {
                min: omin,
                max: omax,
                excl_min: oemin,
                excl_max: oemax,
            },
            Constraint::Range {
                min: nmin,
                max: nmax,
                excl_min: nemin,
                excl_max: nemax,
            },
        ) => {
            raised(*omin, *nmin)
                || lowered(*omax, *nmax)
                || (!oemin && *nemin)
                || (!oemax && *nemax)
        }
        (
            Constraint::Length {
                min: omin,
                max: omax,
            },
            Constraint::Length {
                min: nmin,
                max: nmax,
            },
        ) => {
            raised(omin.map(|v| v as f64), nmin.map(|v| v as f64))
                || lowered(omax.map(|v| v as f64), nmax.map(|v| v as f64))
        }
        (Constraint::Pattern(o), Constraint::Pattern(n)) => o != n,
        (Constraint::MultipleOf(o), Constraint::MultipleOf(n)) => o != n,
        _ => false,
    }
}

/// A lower bound is tighter when it appears where there was none, or moves up.
pub(crate) fn raised(old: Option<f64>, new: Option<f64>) -> bool {
    match (old, new) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n > o,
    }
}

/// An upper bound is tighter when it appears where there was none, or moves down.
pub(crate) fn lowered(old: Option<f64>, new: Option<f64>) -> bool {
    match (old, new) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n < o,
    }
}

/// Multiset equality over type references: order-independent, and each element
/// must appear the same number of times on both sides (so `[X, X, Y]` and
/// `[X, Y, Y]` differ). `Tref` is not hashable, so this counts occurrences.
pub(crate) fn same_set(a: &[Tref], b: &[Tref]) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|t| a.iter().filter(|x| *x == t).count() == b.iter().filter(|x| *x == t).count())
}

/// Whether a shape references a target id from a wire position: struct and
/// union members, operation signatures, and the ops nested in an entry body.
/// Entry/config field targets are construction references, not wire ones.
pub(crate) fn references_on_wire(shape: &Shape, target: &str) -> bool {
    let refs_tref = |t: &Tref| tref_references(t, target);
    match &shape.kind {
        ShapeKind::Structure { members, .. } | ShapeKind::Union { members, .. } => {
            members.iter().any(|m| refs_tref(&m.target))
        }
        ShapeKind::Operation {
            input,
            output,
            errors,
        } => {
            input.as_ref().is_some_and(&refs_tref)
                || output.as_ref().is_some_and(&refs_tref)
                || errors.iter().any(refs_tref)
        }
        ShapeKind::Entry { operations, .. } => {
            operations.iter().any(|op| references_on_wire(op, target))
        }
        ShapeKind::Config { .. } | ShapeKind::Enum { .. } | ShapeKind::Service { .. } => false,
    }
}

pub(crate) fn tref_references(t: &Tref, target: &str) -> bool {
    match t {
        Tref::Ref { id, args } => id == target || args.iter().any(|a| tref_references(a, target)),
        Tref::List(inner) => tref_references(inner, target),
        Tref::Map(k, v) => tref_references(k, target) || tref_references(v, target),
        Tref::Prim(_) | Tref::Param(_) => false,
    }
}

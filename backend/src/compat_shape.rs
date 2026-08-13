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
        // wire carries entry-field paths and literal HTTP names, never a
        // shape id, so it is not a reachability source here.
        ShapeKind::Operation {
            input,
            output,
            errors,
            ..
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Member, Prim};

    fn member(name: &str, target: Tref) -> Member {
        Member {
            name: name.into(),
            target,
            required: true,
            default: None,
            constraints: vec![],
            traits: vec![],
        }
    }

    fn structure(id: &str, members: Vec<Member>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members,
            },
            traits: vec![],
        }
    }

    // ── render_tref / render_opt ────────────────────────────────────────

    #[test]
    fn render_tref_covers_every_variant() {
        assert_eq!(render_tref(&Tref::Prim(Prim::Bool)), "bool");
        assert_eq!(render_tref(&Tref::Param("T".into())), "T");
        assert_eq!(
            render_tref(&Tref::Ref {
                id: "p#Money".into(),
                args: vec![]
            }),
            "p#Money"
        );
        assert_eq!(
            render_tref(&Tref::Ref {
                id: "core#Page".into(),
                args: vec![Tref::Prim(Prim::String)]
            }),
            "core#Page<string>"
        );
        assert_eq!(
            render_tref(&Tref::List(Box::new(Tref::Prim(Prim::I32)))),
            "list<i32>"
        );
        assert_eq!(
            render_tref(&Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::Bool))
            )),
            "map<string, bool>"
        );
    }

    #[test]
    fn render_opt_covers_both_arms() {
        assert_eq!(render_opt(&Some(Tref::Prim(Prim::Bool))), "bool");
        assert_eq!(render_opt(&None), "none");
    }

    // ── kind_name / backing_name / constraint_name ──────────────────────

    #[test]
    fn kind_name_covers_every_variant() {
        assert_eq!(
            kind_name(&ShapeKind::Structure {
                params: vec![],
                members: vec![]
            }),
            "structure"
        );
        assert_eq!(
            kind_name(&ShapeKind::Union {
                params: vec![],
                members: vec![],
                discriminator: "type".into()
            }),
            "union"
        );
        assert_eq!(
            kind_name(&ShapeKind::Enum {
                backing: EnumBacking::String,
                values: vec![]
            }),
            "enum"
        );
        assert_eq!(
            kind_name(&ShapeKind::Service { operations: vec![] }),
            "service"
        );
        assert_eq!(
            kind_name(&ShapeKind::Operation {
                input: None,
                input_name: None,
                output: None,
                errors: vec![],
                wire: None
            }),
            "operation"
        );
        assert_eq!(
            kind_name(&ShapeKind::Entry {
                fields: vec![],
                operations: vec![]
            }),
            "entry"
        );
        assert_eq!(kind_name(&ShapeKind::Config { fields: vec![] }), "config");
    }

    #[test]
    fn backing_name_covers_both_variants() {
        assert_eq!(backing_name(&EnumBacking::String), "string");
        assert_eq!(backing_name(&EnumBacking::Int), "int");
    }

    #[test]
    fn constraint_name_covers_every_variant() {
        assert_eq!(
            constraint_name(&Constraint::Range {
                min: None,
                max: None,
                excl_min: false,
                excl_max: false
            }),
            "range"
        );
        assert_eq!(
            constraint_name(&Constraint::Length {
                min: None,
                max: None
            }),
            "length"
        );
        assert_eq!(constraint_name(&Constraint::Pattern("x".into())), "pattern");
        assert_eq!(constraint_name(&Constraint::MultipleOf(2.0)), "multipleOf");
    }

    // ── tightened / raised / lowered ─────────────────────────────────────

    #[test]
    fn tightened_length_pattern_multipleof_and_mismatch() {
        assert!(tightened(
            &Constraint::Length {
                min: None,
                max: None
            },
            &Constraint::Length {
                min: Some(1),
                max: None
            }
        ));
        assert!(!tightened(
            &Constraint::Length {
                min: Some(1),
                max: None
            },
            &Constraint::Length {
                min: Some(1),
                max: None
            }
        ));
        assert!(tightened(
            &Constraint::Pattern("a".into()),
            &Constraint::Pattern("b".into())
        ));
        assert!(!tightened(
            &Constraint::Pattern("a".into()),
            &Constraint::Pattern("a".into())
        ));
        assert!(tightened(
            &Constraint::MultipleOf(2.0),
            &Constraint::MultipleOf(3.0)
        ));
        assert!(!tightened(
            &Constraint::MultipleOf(2.0),
            &Constraint::MultipleOf(2.0)
        ));
        // mismatched variants fall to the catch-all `_ => false`.
        assert!(!tightened(
            &Constraint::Pattern("a".into()),
            &Constraint::MultipleOf(2.0)
        ));
    }

    #[test]
    fn raised_covers_every_arm() {
        assert!(!raised(Some(1.0), None));
        assert!(raised(None, Some(1.0)));
        assert!(raised(Some(1.0), Some(2.0)));
        assert!(!raised(Some(2.0), Some(1.0)));
    }

    #[test]
    fn lowered_covers_every_arm() {
        assert!(!lowered(Some(1.0), None));
        assert!(lowered(None, Some(1.0)));
        assert!(lowered(Some(2.0), Some(1.0)));
        assert!(!lowered(Some(1.0), Some(2.0)));
    }

    // ── same_set ─────────────────────────────────────────────────────────

    #[test]
    fn same_set_is_order_independent_but_multiplicity_sensitive() {
        let x = Tref::Prim(Prim::Bool);
        let y = Tref::Prim(Prim::String);
        assert!(same_set(&[x.clone(), y.clone()], &[y.clone(), x.clone()]));
        assert!(!same_set(
            &[x.clone(), x.clone(), y.clone()],
            &[x.clone(), y.clone(), y.clone()]
        ));
        assert!(!same_set(std::slice::from_ref(&x), &[x.clone(), y]));
    }

    // ── references_on_wire / tref_references ────────────────────────────

    #[test]
    fn references_on_wire_covers_every_shape_kind() {
        let target = "p#Money";
        let refs = Tref::Ref {
            id: target.into(),
            args: vec![],
        };

        let structure = structure("s#S", vec![member("amount", refs.clone())]);
        assert!(references_on_wire(&structure, target));

        let union = Shape {
            id: "u#U".into(),
            kind: ShapeKind::Union {
                params: vec![],
                members: vec![member("amount", refs.clone())],
                discriminator: "type".into(),
            },
            traits: vec![],
        };
        assert!(references_on_wire(&union, target));

        let op_input = Shape {
            id: "o#in".into(),
            kind: ShapeKind::Operation {
                input: Some(refs.clone()),
                input_name: None,
                output: None,
                errors: vec![],
                wire: None,
            },
            traits: vec![],
        };
        assert!(references_on_wire(&op_input, target));

        let op_output = Shape {
            id: "o#out".into(),
            kind: ShapeKind::Operation {
                input: None,
                input_name: None,
                output: Some(refs.clone()),
                errors: vec![],
                wire: None,
            },
            traits: vec![],
        };
        assert!(references_on_wire(&op_output, target));

        let op_error = Shape {
            id: "o#err".into(),
            kind: ShapeKind::Operation {
                input: None,
                input_name: None,
                output: None,
                errors: vec![refs.clone()],
                wire: None,
            },
            traits: vec![],
        };
        assert!(references_on_wire(&op_error, target));

        let op_none = Shape {
            id: "o#none".into(),
            kind: ShapeKind::Operation {
                input: None,
                input_name: None,
                output: None,
                errors: vec![],
                wire: None,
            },
            traits: vec![],
        };
        assert!(!references_on_wire(&op_none, target));

        let entry = Shape {
            id: "e#E".into(),
            kind: ShapeKind::Entry {
                fields: vec![],
                operations: vec![op_input],
            },
            traits: vec![],
        };
        assert!(references_on_wire(&entry, target));

        let config = Shape {
            id: "c#C".into(),
            kind: ShapeKind::Config { fields: vec![] },
            traits: vec![],
        };
        assert!(!references_on_wire(&config, target));

        let enum_shape = Shape {
            id: "n#N".into(),
            kind: ShapeKind::Enum {
                backing: EnumBacking::String,
                values: vec![],
            },
            traits: vec![],
        };
        assert!(!references_on_wire(&enum_shape, target));

        let service = Shape {
            id: "v#V".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(!references_on_wire(&service, target));
    }

    #[test]
    fn tref_references_covers_every_variant() {
        let target = "p#Money";
        let hit = Tref::Ref {
            id: target.into(),
            args: vec![],
        };
        let miss = Tref::Prim(Prim::Bool);

        assert!(tref_references(&hit, target));
        assert!(!tref_references(&miss, target));
        assert!(tref_references(&Tref::List(Box::new(hit.clone())), target));
        assert!(!tref_references(
            &Tref::List(Box::new(miss.clone())),
            target
        ));
        assert!(tref_references(
            &Tref::Map(Box::new(miss.clone()), Box::new(hit.clone())),
            target
        ));
        assert!(tref_references(
            &Tref::Map(Box::new(hit.clone()), Box::new(miss.clone())),
            target
        ));
        assert!(!tref_references(
            &Tref::Map(Box::new(miss.clone()), Box::new(miss)),
            target
        ));
        assert!(!tref_references(&Tref::Param("T".into()), target));
    }
}

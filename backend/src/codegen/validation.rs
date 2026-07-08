//! The language-neutral constraint-check model every target reuses to turn a
//! member's declared `@range`/`@length` constraints into real validation.
//!
//! A [`Check`] is one atomic guard over a single field: what is compared (the
//! value itself, or its length in the unit its type dictates), the comparison
//! whose truth means the value is INVALID, and the numeric bound. The comparison
//! operators are spelled identically in every current target, so only the
//! length-measuring expression and the wide-integer literal suffix vary per
//! language — supplied through [`ValSyntax`]. The surrounding scaffold (the
//! function/impl shell and how a `Violation` is constructed) stays with each
//! target, since those idioms genuinely differ.
//!
//! Two scoping rules keep the emission honest rather than half-done: a constraint
//! on an OPTIONAL member is skipped (guarding an absent value is a separate,
//! per-language concern, deferred), and `@pattern` (which would pull a regex
//! engine as a dependency) and `@multipleOf` (unexercised) are not lowered yet.

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::field_ident;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr};
use crate::ir::{Constraint, Member, Prim, Tref};

/// What a length check counts, since the unit differs by the field's type and each
/// target measures it in its own idiom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Measure {
    /// Unicode code points of a string.
    Chars,
    /// Elements of a list or entries of a map.
    Elements,
    /// Bytes of a `bytes` value.
    Bytes,
}

/// The comparison whose truth means the value is INVALID. The four operators are
/// spelled the same in every current target, so the symbol is shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
}

impl Cmp {
    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
        }
    }
}

/// What a check compares: the field's own value (a numeric bound, carrying whether
/// the field is a 64-bit integer so a target can pick the wide literal form), or
/// the field's length in a given unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Value { wide: bool },
    Length(Measure),
}

/// One atomic validation guard over a single field. `field` is the canonical
/// (wire) name carried into `Violation.field`; `constraint` is the category
/// (`"range"`/`"length"`); `message` is a human-readable explanation.
#[derive(Debug, Clone, PartialEq)]
pub struct Check {
    pub field: String,
    pub constraint: &'static str,
    pub message: String,
    pub subject: Subject,
    pub cmp: Cmp,
    pub bound: f64,
}

/// The per-language spellings a check needs to render its condition: how to
/// measure a length, and the suffix a 64-bit integer literal carries (TypeScript's
/// `bigint` needs `n`; native-int languages need nothing).
pub trait ValSyntax {
    fn length(&self, access: &str, measure: Measure) -> String;
    fn wide_suffix(&self) -> &str;
}

impl Check {
    /// The boolean condition that is true exactly when the field is invalid, over
    /// the target's field-access expression.
    pub fn condition(&self, access: &str, syntax: &dyn ValSyntax) -> String {
        let (lhs, rhs) = match self.subject {
            Subject::Value { wide } => {
                let suffix = if wide { syntax.wide_suffix() } else { "" };
                (
                    access.to_string(),
                    format!("{}{suffix}", fmt_bound(self.bound)),
                )
            }
            Subject::Length(measure) => (syntax.length(access, measure), fmt_bound(self.bound)),
        };
        format!("{lhs} {} {rhs}", self.cmp.symbol())
    }
}

/// Render a bound without a needless fractional part: an integral `f64` (every
/// bound the frontend produces for an integer or length) prints as an integer, so
/// `0.0` becomes `0`.
fn fmt_bound(v: f64) -> String {
    if v.is_finite() && v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// Whether a member's own type is a 64-bit integer, which some targets hold as a
/// distinct numeric type (a `bigint`) and must bound with a matching literal.
fn is_wide_int(t: &Tref) -> bool {
    matches!(t, Tref::Prim(Prim::I64) | Tref::Prim(Prim::U64))
}

/// The length unit a `@length` constraint measures for a member's type, or `None`
/// for a type `@length` does not apply to (which the frontend rejects upstream).
fn measure_of(t: &Tref) -> Option<Measure> {
    match t {
        Tref::Prim(Prim::String) | Tref::Prim(Prim::Uuid) => Some(Measure::Chars),
        Tref::Prim(Prim::Bytes) => Some(Measure::Bytes),
        Tref::List(_) | Tref::Map(_, _) => Some(Measure::Elements),
        _ => None,
    }
}

/// The checks a member's declared constraints lower to. An optional member yields
/// none (guarding an absent value is deferred), and `@pattern`/`@multipleOf` are
/// not lowered yet; a range or length bound each becomes one comparison whose
/// truth means the value is out of bounds.
pub fn member_checks(member: &Member) -> Vec<Check> {
    if !member.required {
        return Vec::new();
    }
    let field = member.name.clone();
    let mut out = Vec::new();
    for constraint in &member.constraints {
        match constraint {
            Constraint::Range {
                min,
                max,
                excl_min,
                excl_max,
            } => {
                let wide = is_wide_int(&member.target);
                if let Some(min) = min {
                    // A value below the minimum is invalid: `< min` (inclusive) or
                    // `<= min` (exclusive, since equality is not allowed).
                    out.push(Check {
                        field: field.clone(),
                        constraint: "range",
                        message: format!(
                            "{field} must be {} {}",
                            if *excl_min { ">" } else { ">=" },
                            fmt_bound(*min)
                        ),
                        subject: Subject::Value { wide },
                        cmp: if *excl_min { Cmp::Le } else { Cmp::Lt },
                        bound: *min,
                    });
                }
                if let Some(max) = max {
                    out.push(Check {
                        field: field.clone(),
                        constraint: "range",
                        message: format!(
                            "{field} must be {} {}",
                            if *excl_max { "<" } else { "<=" },
                            fmt_bound(*max)
                        ),
                        subject: Subject::Value { wide },
                        cmp: if *excl_max { Cmp::Ge } else { Cmp::Gt },
                        bound: *max,
                    });
                }
            }
            Constraint::Length { min, max } => {
                let Some(measure) = measure_of(&member.target) else {
                    continue;
                };
                if let Some(min) = min {
                    out.push(Check {
                        field: field.clone(),
                        constraint: "length",
                        message: format!("{field} length must be >= {min}"),
                        subject: Subject::Length(measure),
                        cmp: Cmp::Lt,
                        bound: *min as f64,
                    });
                }
                if let Some(max) = max {
                    out.push(Check {
                        field: field.clone(),
                        constraint: "length",
                        message: format!("{field} length must be <= {max}"),
                        subject: Subject::Length(measure),
                        cmp: Cmp::Gt,
                        bound: *max as f64,
                    });
                }
            }
            // A pattern check would pull a regex engine as a dependency, and
            // `@multipleOf` is unexercised; both are left for a future need.
            Constraint::Pattern(_) | Constraint::MultipleOf(_) => {}
        }
    }
    out
}

/// Whether a shape is a structure whose members carry any lowerable constraint,
/// i.e. whether it needs a validator emitted at all. A generic structure is
/// excluded (a validator over its type parameters is not modeled yet).
pub fn shape_has_checks(shape: &crate::ir::Shape) -> bool {
    match &shape.kind {
        crate::ir::ShapeKind::Structure { params, members } if params.is_empty() => {
            members.iter().any(|m| !member_checks(m).is_empty())
        }
        _ => false,
    }
}

/// One rendered guard a validator emits: the boolean condition (already spelled
/// through the target's syntax over the field access) plus the `Violation` triple
/// to push when it holds. The target only formats these into its own statement.
#[derive(Debug, Clone, PartialEq)]
pub struct GuardLine {
    pub condition: String,
    pub field: String,
    pub constraint: &'static str,
    pub message: String,
}

/// The guard lines for a structure's members: each check rendered against
/// `<access_prefix><field-ident>` (e.g. `self.amount`, `value.Currency`), so the
/// per-member/per-check iteration and condition building live here rather than in
/// each target. Empty when nothing is constrained.
pub fn guard_lines(
    members: &[Member],
    syntax: &dyn ValSyntax,
    access_prefix: &str,
    config: &CasingConfig,
    lang: &str,
) -> Vec<GuardLine> {
    let mut out = Vec::new();
    for member in members {
        let access = format!("{access_prefix}{}", field_ident(member, config, lang));
        for check in member_checks(member) {
            out.push(GuardLine {
                condition: check.condition(&access, syntax),
                field: check.field.clone(),
                constraint: check.constraint,
                message: check.message.clone(),
            });
        }
    }
    out
}

/// The guard lines for a shape that should get a validator, or `None` when it
/// should not: a non-structure, a generic structure (unmodeled), or a structure
/// with no lowerable constraint. Collapses the shared preamble every target's
/// `emit_validators` would otherwise repeat.
pub fn structure_guard_lines(
    shape: &crate::ir::Shape,
    syntax: &dyn ValSyntax,
    access_prefix: &str,
    config: &CasingConfig,
    lang: &str,
) -> Option<Vec<GuardLine>> {
    let crate::ir::ShapeKind::Structure { params, members } = &shape.kind else {
        return None;
    };
    if !params.is_empty() {
        return None;
    }
    let lines = guard_lines(members, syntax, access_prefix, config, lang);
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
}

/// Assemble a validator body from its guard lines: a header, one rendered
/// statement per guard (the target supplies the `if`/push spelling), and a footer.
pub fn validator_body(
    lines: &[GuardLine],
    header: &str,
    footer: &str,
    guard: impl Fn(&GuardLine) -> String,
) -> String {
    let mut body = header.to_string();
    for line in lines {
        body.push_str(&guard(line));
    }
    body.push_str(footer);
    body
}

/// A free-function validator declaration shared by the targets that spell one that
/// way (TypeScript, Go): `fn(value: <Ty>) -> Vec<Violation>` over the given body.
/// Rust instead emits an inherent method, so it builds its own declaration.
pub fn validator_function(name: String, ty: String, violation: String, body: String) -> Decl {
    Decl::Function(Function {
        name: Symbol::builtin(name),
        params: vec![Field {
            name: Symbol::builtin("value"),
            ty: TypeExpr::Ref(Symbol::builtin(ty)),
            nullable: false,
            wire: None,
            deprecated: None,
        }],
        ret: Some(TypeExpr::list(TypeExpr::Ref(Symbol::builtin(violation)))),
        body: FnBody::Raw {
            text: body,
            refs: Vec::new(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, target: Tref, required: bool, constraints: Vec<Constraint>) -> Member {
        Member {
            name: name.into(),
            target,
            required,
            default: None,
            constraints,
            traits: vec![],
        }
    }

    struct FakeSyntax;
    impl ValSyntax for FakeSyntax {
        fn length(&self, access: &str, measure: Measure) -> String {
            match measure {
                Measure::Chars => format!("chars({access})"),
                Measure::Elements => format!("len({access})"),
                Measure::Bytes => format!("bytes({access})"),
            }
        }
        fn wide_suffix(&self) -> &str {
            "n"
        }
    }

    #[test]
    fn an_inclusive_range_min_is_invalid_below_the_bound() {
        let m = member(
            "amount",
            Tref::Prim(Prim::I64),
            true,
            vec![Constraint::Range {
                min: Some(0.0),
                max: None,
                excl_min: false,
                excl_max: false,
            }],
        );
        let checks = member_checks(&m);
        assert_eq!(checks.len(), 1);
        let c = &checks[0];
        assert_eq!(c.constraint, "range");
        assert_eq!(c.message, "amount must be >= 0");
        // i64 is wide, so the literal carries the target's wide suffix.
        assert_eq!(
            c.condition("value.amount", &FakeSyntax),
            "value.amount < 0n"
        );
    }

    #[test]
    fn an_exclusive_range_flips_the_operator_and_the_message() {
        let m = member(
            "ratio",
            Tref::Prim(Prim::Float),
            true,
            vec![Constraint::Range {
                min: Some(0.0),
                max: Some(1.0),
                excl_min: true,
                excl_max: true,
            }],
        );
        let checks = member_checks(&m);
        assert_eq!(checks.len(), 2);
        // A float is not wide, so no suffix; exclusive min is violated at equality.
        assert_eq!(checks[0].message, "ratio must be > 0");
        assert_eq!(checks[0].condition("v", &FakeSyntax), "v <= 0");
        assert_eq!(checks[1].message, "ratio must be < 1");
        assert_eq!(checks[1].condition("v", &FakeSyntax), "v >= 1");
    }

    #[test]
    fn a_length_constraint_measures_by_type_and_bounds_both_ends() {
        let m = member(
            "currency",
            Tref::Prim(Prim::String),
            true,
            vec![Constraint::Length {
                min: Some(3),
                max: Some(3),
            }],
        );
        let checks = member_checks(&m);
        assert_eq!(checks.len(), 2);
        assert_eq!(checks[0].constraint, "length");
        assert_eq!(checks[0].message, "currency length must be >= 3");
        assert_eq!(
            checks[0].condition("v.currency", &FakeSyntax),
            "chars(v.currency) < 3"
        );
        assert_eq!(
            checks[1].condition("v.currency", &FakeSyntax),
            "chars(v.currency) > 3"
        );
    }

    #[test]
    fn a_list_length_measures_elements_and_bytes_measure_bytes() {
        let list = member(
            "tags",
            Tref::List(Box::new(Tref::Prim(Prim::String))),
            true,
            vec![Constraint::Length {
                min: Some(1),
                max: None,
            }],
        );
        assert_eq!(
            member_checks(&list)[0].condition("v.tags", &FakeSyntax),
            "len(v.tags) < 1"
        );
        let blob = member(
            "receipt",
            Tref::Prim(Prim::Bytes),
            true,
            vec![Constraint::Length {
                min: None,
                max: Some(1024),
            }],
        );
        assert_eq!(
            member_checks(&blob)[0].condition("v.receipt", &FakeSyntax),
            "bytes(v.receipt) > 1024"
        );
    }

    #[test]
    fn an_optional_member_and_the_deferred_constraints_yield_no_checks() {
        // An optional member is skipped wholesale.
        let optional = member(
            "note",
            Tref::Prim(Prim::String),
            false,
            vec![Constraint::Length {
                min: Some(1),
                max: None,
            }],
        );
        assert!(member_checks(&optional).is_empty());
        // Pattern and multipleOf are not lowered yet.
        let deferred = member(
            "code",
            Tref::Prim(Prim::String),
            true,
            vec![
                Constraint::Pattern("^x$".into()),
                Constraint::MultipleOf(2.0),
            ],
        );
        assert!(member_checks(&deferred).is_empty());
    }
}

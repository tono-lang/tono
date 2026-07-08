//! Compatibility checker: diff a baseline IR against the current one and classify
//! every change into a break taxonomy, then apply a configurable severity per
//! level plus an allowlist for deliberate breaks.
//!
//! The module is pure and IO-free: it takes two decoded [`Model`]s and a
//! [`Config`], and returns a [`Report`]. Resolving the baseline (a git ref) and
//! printing the report are the CLI's job. The classification mirrors the wire /
//! source compat rules the IR already encodes: an enum is always open (so adding
//! a value is safe), nullability rides on a member's `required` flag, and an
//! `i64` travels as a string, so retyping any member is a wire break.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::ir::{Constraint, EnumBacking, Member, Model, Shape, ShapeKind, Trait, Tref};

/// The break level a change falls into. The order is severity-ascending only for
/// tie-breaking display; the effective severity comes from [`Config`], not from
/// this ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Category {
    /// Breaks the wire contract: existing encoded data no longer round-trips.
    WireBreaking,
    /// Breaks generated source: a consumer's code no longer compiles.
    SourceBreaking,
    /// Compiles and round-trips, but changes runtime meaning (a default or a
    /// tightened constraint).
    Behavioral,
    /// Safe to ship: additive and backward-compatible.
    AdditiveSafe,
}

impl Category {
    /// The out-of-the-box severity for a level (RFC-0012 defaults). Overridable
    /// per level in [`Config`].
    pub fn default_severity(self) -> Severity {
        match self {
            Category::WireBreaking | Category::SourceBreaking => Severity::Error,
            Category::Behavioral => Severity::Warn,
            Category::AdditiveSafe => Severity::Off,
        }
    }

    /// The kebab-case level name used on the command line and in reports.
    pub fn label(self) -> &'static str {
        match self {
            Category::WireBreaking => "wire-breaking",
            Category::SourceBreaking => "source-breaking",
            Category::Behavioral => "behavioral",
            Category::AdditiveSafe => "additive-safe",
        }
    }

    /// Parse a level name as accepted on the command line (kebab-case).
    pub fn parse(name: &str) -> Option<Self> {
        [
            Category::WireBreaking,
            Category::SourceBreaking,
            Category::Behavioral,
            Category::AdditiveSafe,
        ]
        .into_iter()
        .find(|c| c.label() == name)
    }
}

/// How a level is enforced. `Error` fails the gate, `Warn` reports, `Off` ignores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warn,
    Off,
}

impl Severity {
    /// A rank so the worst severity across a report is a simple `max`.
    fn rank(self) -> u8 {
        match self {
            Severity::Off => 0,
            Severity::Warn => 1,
            Severity::Error => 2,
        }
    }

    /// The name used on the command line and in reports.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warn => "warn",
            Severity::Off => "off",
        }
    }

    /// Parse a severity name as accepted on the command line.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "error" => Some(Severity::Error),
            "warn" | "warning" => Some(Severity::Warn),
            "off" => Some(Severity::Off),
            _ => None,
        }
    }
}

/// One classified difference between the two models. `key` is a stable,
/// human-readable identifier an allowlist entry matches exactly; `category` is
/// intrinsic to the diff and never mutated by config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub key: String,
    pub category: Category,
    pub detail: String,
}

/// The enforcement policy: a per-level severity override and an allowlist of
/// change keys to suppress. Deserializes from JSON, e.g.
/// `{"severity":{"wireBreaking":"warn"},"allow":["remove-member a#B.c"]}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub severity: BTreeMap<Category, Severity>,
    pub allow: Vec<String>,
}

impl Config {
    /// Parse a config from its JSON form, e.g.
    /// `{"severity":{"wireBreaking":"warn"},"allow":["remove-member a#B.c"]}`.
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| e.to_string())
    }

    /// The effective severity of a change: `Off` if allowlisted, else the
    /// configured override for its category, else the category default.
    pub fn severity_of(&self, change: &Change) -> Severity {
        if self.allow.iter().any(|a| a == &change.key) {
            return Severity::Off;
        }
        self.severity
            .get(&change.category)
            .copied()
            .unwrap_or_else(|| change.category.default_severity())
    }
}

/// The classified diff between a baseline and the current model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub changes: Vec<Change>,
}

impl Report {
    /// The worst effective severity across all changes under `config`. `Off` when
    /// nothing is reportable, so an empty or fully-allowlisted diff passes.
    pub fn worst(&self, config: &Config) -> Severity {
        self.changes
            .iter()
            .map(|c| config.severity_of(c))
            .max_by_key(|s| s.rank())
            .unwrap_or(Severity::Off)
    }
}

/// Diff `baseline` against `current`, classifying every difference. Deterministic:
/// shapes and members are compared over sorted id / name sets, so the report order
/// is stable regardless of input order.
pub fn diff(baseline: &Model, current: &Model) -> Report {
    let base = index(baseline);
    let curr = index(current);

    let mut ids: Vec<&str> = base.keys().chain(curr.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();

    let mut out = Vec::new();
    for id in ids {
        match (base.get(id), curr.get(id)) {
            (Some(b), None) => out.push(removed_shape(b, &curr)),
            (None, Some(c)) => added_shape(c, &mut out),
            (Some(b), Some(c)) => diff_shape(b, c, &mut out),
            (None, None) => unreachable!("id came from one of the two maps"),
        }
    }
    Report { changes: out }
}

/// Fold a model's shapes and operations into one id-keyed index. Operations live
/// in a separate vec but are themselves shapes; ids are globally unique namespaced
/// strings, so a flat cross-module index is correct.
fn index(model: &Model) -> BTreeMap<&str, &Shape> {
    let mut m = BTreeMap::new();
    for module in &model.modules {
        for shape in module.shapes.iter().chain(module.operations.iter()) {
            m.insert(shape.id.as_str(), shape);
        }
    }
    m
}

/// A removed shape. Still referenced by a surviving shape means encoded data
/// points at a type that is gone (wire break); otherwise it is a vanished public
/// declaration (source break).
fn removed_shape(shape: &Shape, current: &BTreeMap<&str, &Shape>) -> Change {
    let referenced = current.values().any(|s| references(s, &shape.id));
    let category = if referenced {
        Category::WireBreaking
    } else {
        Category::SourceBreaking
    };
    Change {
        key: format!("remove-shape {}", shape.id),
        category,
        detail: format!("{} removed", kind_name(&shape.kind)),
    }
}

/// A newly-added shape is additive; a `@deprecated` it carries is additive too.
fn added_shape(shape: &Shape, out: &mut Vec<Change>) {
    out.push(Change {
        key: format!("add-shape {}", shape.id),
        category: Category::AdditiveSafe,
        detail: format!("{} added", kind_name(&shape.kind)),
    });
    if is_deprecated(&shape.traits) {
        out.push(deprecation_change(&shape.id));
    }
}

fn diff_shape(b: &Shape, c: &Shape, out: &mut Vec<Change>) {
    match (&b.kind, &c.kind) {
        (
            ShapeKind::Structure {
                params: bp,
                members: bm,
            },
            ShapeKind::Structure {
                params: cp,
                members: cm,
            },
        ) => {
            diff_params(&b.id, bp, cp, out);
            diff_members(&b.id, bm, cm, out);
        }
        (
            ShapeKind::Union {
                params: bp,
                members: bm,
                discriminator: bd,
            },
            ShapeKind::Union {
                params: cp,
                members: cm,
                discriminator: cd,
            },
        ) => {
            diff_params(&b.id, bp, cp, out);
            if bd != cd {
                out.push(Change {
                    key: format!("change-discriminator {}@discriminator", b.id),
                    category: Category::WireBreaking,
                    detail: format!("{bd} -> {cd}"),
                });
            }
            diff_members(&b.id, bm, cm, out);
        }
        (
            ShapeKind::Enum {
                backing: bb,
                values: bv,
            },
            ShapeKind::Enum {
                backing: cb,
                values: cv,
            },
        ) => diff_enum(&b.id, bb, bv, cb, cv, out),
        (
            ShapeKind::Operation {
                input: bi,
                output: bo,
                errors: be,
            },
            ShapeKind::Operation {
                input: ci,
                output: co,
                errors: ce,
            },
        ) => diff_operation(&b.id, bi, bo, be, ci, co, ce, out),
        (ShapeKind::Service { operations: bo }, ShapeKind::Service { operations: co }) => {
            diff_service(&b.id, bo, co, out)
        }
        // A shape whose kind changed (structure -> enum, ...) is not a smooth
        // diff: the old kind is gone and a new one takes its id.
        _ => out.push(Change {
            key: format!("remove-shape {}", b.id),
            category: Category::WireBreaking,
            detail: format!("{} -> {}", kind_name(&b.kind), kind_name(&c.kind)),
        }),
    }
    if !is_deprecated(&b.traits) && is_deprecated(&c.traits) {
        out.push(deprecation_change(&b.id));
    }
}

/// A generic shape's type-parameter list. A change to it alters the shape's arity,
/// so every instantiation in generated source breaks.
fn diff_params(id: &str, base: &[String], curr: &[String], out: &mut Vec<Change>) {
    if base != curr {
        out.push(Change {
            key: format!("change-params {id}"),
            category: Category::SourceBreaking,
            detail: format!("[{}] -> [{}]", base.join(", "), curr.join(", ")),
        });
    }
}

/// Compare members by name (order is irrelevant). A removed member is a wire
/// break; an added one is safe unless it is required with no default (an existing
/// producer would omit it).
fn diff_members(id: &str, base: &[Member], curr: &[Member], out: &mut Vec<Change>) {
    let bi: BTreeMap<&str, &Member> = base.iter().map(|m| (m.name.as_str(), m)).collect();
    let ci: BTreeMap<&str, &Member> = curr.iter().map(|m| (m.name.as_str(), m)).collect();

    let mut names: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();

    for name in names {
        match (bi.get(name), ci.get(name)) {
            (Some(b), None) => out.push(Change {
                key: format!("remove-member {id}.{name}"),
                category: Category::WireBreaking,
                detail: format!("{name}: {} removed", render_tref(&b.target)),
            }),
            (None, Some(c)) => {
                let safe = !c.required || c.default.is_some();
                out.push(Change {
                    key: format!("add-member {id}.{name}"),
                    category: if safe {
                        Category::AdditiveSafe
                    } else {
                        Category::WireBreaking
                    },
                    detail: format!(
                        "{name}: {} added{}",
                        render_tref(&c.target),
                        if safe { "" } else { " (required, no default)" }
                    ),
                });
            }
            (Some(b), Some(c)) => diff_member(id, b, c, out),
            (None, None) => unreachable!("name came from one of the two maps"),
        }
    }
}

fn diff_member(id: &str, b: &Member, c: &Member, out: &mut Vec<Change>) {
    let path = format!("{id}.{}", b.name);

    if b.target != c.target {
        out.push(Change {
            key: format!("retype {path}"),
            category: Category::WireBreaking,
            detail: format!("{} -> {}", render_tref(&b.target), render_tref(&c.target)),
        });
    }

    match (b.required, c.required) {
        (false, true) => out.push(Change {
            key: format!("require-member {path}"),
            category: Category::WireBreaking,
            detail: "optional -> required".into(),
        }),
        (true, false) => out.push(Change {
            key: format!("optional-member {path}"),
            category: Category::AdditiveSafe,
            detail: "required -> optional".into(),
        }),
        _ => {}
    }

    if wire_of(&b.traits) != wire_of(&c.traits) {
        out.push(Change {
            key: format!("change-wire {path}@wire"),
            category: Category::WireBreaking,
            detail: format!(
                "{} -> {}",
                wire_of(&b.traits).unwrap_or(&b.name),
                wire_of(&c.traits).unwrap_or(&c.name)
            ),
        });
    }

    if b.default != c.default {
        out.push(Change {
            key: format!("change-default {path}"),
            category: Category::Behavioral,
            detail: "default value changed".into(),
        });
    }

    diff_constraints(&path, &b.constraints, &c.constraints, out);
}

/// A new constraint, or one tightened to reject previously-valid values, changes
/// runtime behavior. Pure loosening is backward-compatible and emits nothing.
fn diff_constraints(path: &str, base: &[Constraint], curr: &[Constraint], out: &mut Vec<Change>) {
    for cc in curr {
        match base.iter().find(|bc| same_constraint_kind(bc, cc)) {
            None => out.push(Change {
                key: format!("add-constraint {path}"),
                category: Category::Behavioral,
                detail: format!("added {} constraint", constraint_name(cc)),
            }),
            Some(bc) if tightened(bc, cc) => out.push(Change {
                key: format!("tighten-constraint {path}"),
                category: Category::Behavioral,
                detail: format!("tightened {} constraint", constraint_name(cc)),
            }),
            Some(_) => {}
        }
    }
}

/// Compare enum values by tag. A removed value narrows the type (wire break); an
/// added value is safe because enums are always open; a changed integer
/// discriminant re-points an existing value on the wire.
fn diff_enum(
    id: &str,
    bb: &EnumBacking,
    bv: &[(String, Option<i64>)],
    cb: &EnumBacking,
    cv: &[(String, Option<i64>)],
    out: &mut Vec<Change>,
) {
    if bb != cb {
        out.push(Change {
            key: format!("change-enum-backing {id}"),
            category: Category::WireBreaking,
            detail: format!("{} -> {}", backing_name(bb), backing_name(cb)),
        });
    }

    let bi: BTreeMap<&str, &Option<i64>> = bv.iter().map(|(t, n)| (t.as_str(), n)).collect();
    let ci: BTreeMap<&str, &Option<i64>> = cv.iter().map(|(t, n)| (t.as_str(), n)).collect();

    let mut tags: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    tags.sort_unstable();
    tags.dedup();

    for tag in tags {
        match (bi.get(tag), ci.get(tag)) {
            (Some(_), None) => out.push(Change {
                key: format!("remove-enum-value {id}::{tag}"),
                category: Category::WireBreaking,
                detail: format!("value {tag} removed"),
            }),
            (None, Some(_)) => out.push(Change {
                key: format!("add-enum-value {id}::{tag}"),
                category: Category::AdditiveSafe,
                detail: format!("value {tag} added"),
            }),
            (Some(bn), Some(cn)) if bn != cn => out.push(Change {
                key: format!("change-enum-value {id}::{tag}"),
                category: Category::WireBreaking,
                detail: format!("{tag}: {bn:?} -> {cn:?}"),
            }),
            _ => {}
        }
    }
}

/// An operation's signature: input, output, and error set. Any change breaks the
/// generated call site (source break); if an input/output type was removed, the
/// removed-shape pass separately records the wire break.
#[allow(clippy::too_many_arguments)]
fn diff_operation(
    id: &str,
    bi: &Option<Tref>,
    bo: &Option<Tref>,
    be: &[Tref],
    ci: &Option<Tref>,
    co: &Option<Tref>,
    ce: &[Tref],
    out: &mut Vec<Change>,
) {
    if bi != ci {
        out.push(Change {
            key: format!("change-operation {id}$input"),
            category: Category::SourceBreaking,
            detail: format!("input {} -> {}", render_opt(bi), render_opt(ci)),
        });
    }
    if bo != co {
        out.push(Change {
            key: format!("change-operation {id}$output"),
            category: Category::SourceBreaking,
            detail: format!("output {} -> {}", render_opt(bo), render_opt(co)),
        });
    }
    if !same_set(be, ce) {
        out.push(Change {
            key: format!("change-operation {id}$errors"),
            category: Category::SourceBreaking,
            detail: "error set changed".into(),
        });
    }
}

/// A service's operation list, compared as a set. Removing an operation drops it
/// from the generated client (source break); adding one is safe.
fn diff_service(id: &str, base: &[String], curr: &[String], out: &mut Vec<Change>) {
    for op in base {
        if !curr.contains(op) {
            out.push(Change {
                key: format!("remove-service-op {id}/{op}"),
                category: Category::SourceBreaking,
                detail: format!("operation {op} removed"),
            });
        }
    }
    for op in curr {
        if !base.contains(op) {
            out.push(Change {
                key: format!("add-service-op {id}/{op}"),
                category: Category::AdditiveSafe,
                detail: format!("operation {op} added"),
            });
        }
    }
}

fn deprecation_change(id: &str) -> Change {
    Change {
        key: format!("add-deprecated {id}"),
        category: Category::AdditiveSafe,
        detail: "marked deprecated".into(),
    }
}

// --- small readers and renderers, kept local so the module stands alone ---

/// The `@wire` override on a member (trait `core#wire`), or `None`.
fn wire_of(traits: &[Trait]) -> Option<&str> {
    traits
        .iter()
        .find(|t| t.id == "core#wire")
        .and_then(|t| t.value.as_str())
}

/// Whether a `@deprecated` trait (`core#deprecated`) is present.
fn is_deprecated(traits: &[Trait]) -> bool {
    traits.iter().any(|t| t.id == "core#deprecated")
}

/// A compact, human-readable spelling of a type reference for change details.
fn render_tref(t: &Tref) -> String {
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

fn render_opt(t: &Option<Tref>) -> String {
    match t {
        Some(t) => render_tref(t),
        None => "none".into(),
    }
}

fn kind_name(kind: &ShapeKind) -> &'static str {
    match kind {
        ShapeKind::Structure { .. } => "structure",
        ShapeKind::Union { .. } => "union",
        ShapeKind::Enum { .. } => "enum",
        ShapeKind::Service { .. } => "service",
        ShapeKind::Operation { .. } => "operation",
    }
}

fn backing_name(b: &EnumBacking) -> &'static str {
    match b {
        EnumBacking::String => "string",
        EnumBacking::Int => "int",
    }
}

fn constraint_name(c: &Constraint) -> &'static str {
    match c {
        Constraint::Range { .. } => "range",
        Constraint::Length { .. } => "length",
        Constraint::Pattern(_) => "pattern",
        Constraint::MultipleOf(_) => "multipleOf",
    }
}

/// Whether two constraints are the same variant (so they pair up for a tightening
/// comparison rather than reading as an add/remove).
fn same_constraint_kind(a: &Constraint, b: &Constraint) -> bool {
    constraint_name(a) == constraint_name(b)
}

/// Whether `new` rejects values `old` accepted. Bounds narrowing (a raised min, a
/// lowered max, or an inclusive bound made exclusive) tightens; `Pattern` and
/// `MultipleOf` are treated conservatively (any change tightens) since proving a
/// looser regex or modulus dependency-free is not worth it.
fn tightened(old: &Constraint, new: &Constraint) -> bool {
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
fn raised(old: Option<f64>, new: Option<f64>) -> bool {
    match (old, new) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n > o,
    }
}

/// An upper bound is tighter when it appears where there was none, or moves down.
fn lowered(old: Option<f64>, new: Option<f64>) -> bool {
    match (old, new) {
        (_, None) => false,
        (None, Some(_)) => true,
        (Some(o), Some(n)) => n < o,
    }
}

/// Set equality over type references (order-independent, allowing duplicates).
fn same_set(a: &[Tref], b: &[Tref]) -> bool {
    a.len() == b.len() && a.iter().all(|t| b.contains(t)) && b.iter().all(|t| a.contains(t))
}

/// Whether a shape's members / operation signature reference a target id.
fn references(shape: &Shape, target: &str) -> bool {
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
        ShapeKind::Enum { .. } | ShapeKind::Service { .. } => false,
    }
}

fn tref_references(t: &Tref, target: &str) -> bool {
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
    use crate::ir::{Module, Prim};
    use serde_json::json;

    fn model(shapes: Vec<Shape>) -> Model {
        Model {
            tono_ir_version: 2,
            modules: vec![Module {
                name: "billing".into(),
                shapes,
                operations: vec![],
            }],
        }
    }

    fn member(name: &str, target: Tref, required: bool) -> Member {
        Member {
            name: name.into(),
            target,
            required,
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

    fn charge(members: Vec<Member>) -> Model {
        model(vec![structure("billing#Charge", members)])
    }

    fn keys(report: &Report) -> Vec<&str> {
        report.changes.iter().map(|c| c.key.as_str()).collect()
    }

    fn find<'a>(report: &'a Report, key: &str) -> &'a Change {
        report
            .changes
            .iter()
            .find(|c| c.key == key)
            .unwrap_or_else(|| panic!("no change with key {key:?} in {:?}", keys(report)))
    }

    #[test]
    fn identical_models_have_no_changes() {
        let m = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        assert!(diff(&m, &m).changes.is_empty());
    }

    #[test]
    fn retyping_a_member_is_wire_breaking() {
        let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        let after = charge(vec![member("amount", Tref::Prim(Prim::String), true)]);
        let report = diff(&before, &after);
        let change = find(&report, "retype billing#Charge.amount");
        assert_eq!(change.category, Category::WireBreaking);
        assert_eq!(change.detail, "u64 -> string");
    }

    #[test]
    fn removing_a_member_is_wire_breaking() {
        let before = charge(vec![
            member("amount", Tref::Prim(Prim::U64), true),
            member("note", Tref::Prim(Prim::String), false),
        ]);
        let after = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        assert_eq!(
            find(&diff(&before, &after), "remove-member billing#Charge.note").category,
            Category::WireBreaking
        );
    }

    #[test]
    fn adding_an_optional_member_is_safe_but_a_required_one_breaks() {
        let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        let with_optional = charge(vec![
            member("amount", Tref::Prim(Prim::U64), true),
            member("note", Tref::Prim(Prim::String), false),
        ]);
        assert_eq!(
            find(
                &diff(&before, &with_optional),
                "add-member billing#Charge.note"
            )
            .category,
            Category::AdditiveSafe
        );

        let with_required = charge(vec![
            member("amount", Tref::Prim(Prim::U64), true),
            member("note", Tref::Prim(Prim::String), true),
        ]);
        assert_eq!(
            find(
                &diff(&before, &with_required),
                "add-member billing#Charge.note"
            )
            .category,
            Category::WireBreaking
        );
    }

    #[test]
    fn making_a_member_required_breaks_but_optional_is_safe() {
        let optional = charge(vec![member("note", Tref::Prim(Prim::String), false)]);
        let required = charge(vec![member("note", Tref::Prim(Prim::String), true)]);
        assert_eq!(
            find(
                &diff(&optional, &required),
                "require-member billing#Charge.note"
            )
            .category,
            Category::WireBreaking
        );
        assert_eq!(
            find(
                &diff(&required, &optional),
                "optional-member billing#Charge.note"
            )
            .category,
            Category::AdditiveSafe
        );
    }

    #[test]
    fn changing_a_wire_key_is_wire_breaking() {
        let mut renamed = member("amount", Tref::Prim(Prim::U64), true);
        renamed.traits = vec![Trait {
            id: "core#wire".into(),
            value: json!("amount_cents"),
        }];
        let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        let after = charge(vec![renamed]);
        assert_eq!(
            find(
                &diff(&before, &after),
                "change-wire billing#Charge.amount@wire"
            )
            .category,
            Category::WireBreaking
        );
    }

    #[test]
    fn changing_a_default_is_behavioral() {
        let mut before_m = member("amount", Tref::Prim(Prim::U64), true);
        before_m.default = Some(Some(json!(0)));
        let mut after_m = member("amount", Tref::Prim(Prim::U64), true);
        after_m.default = Some(Some(json!(1)));
        assert_eq!(
            find(
                &diff(&charge(vec![before_m]), &charge(vec![after_m])),
                "change-default billing#Charge.amount"
            )
            .category,
            Category::Behavioral
        );
    }

    #[test]
    fn tightening_a_range_is_behavioral_but_loosening_is_silent() {
        let with_max = |max: f64| {
            let mut m = member("amount", Tref::Prim(Prim::U64), true);
            m.constraints = vec![Constraint::Range {
                min: None,
                max: Some(max),
                excl_min: false,
                excl_max: false,
            }];
            charge(vec![m])
        };
        assert_eq!(
            find(
                &diff(&with_max(100.0), &with_max(50.0)),
                "tighten-constraint billing#Charge.amount"
            )
            .category,
            Category::Behavioral
        );
        // Loosening (raising the max) emits nothing.
        assert!(diff(&with_max(50.0), &with_max(100.0)).changes.is_empty());
    }

    #[test]
    fn adding_a_constraint_is_behavioral() {
        let plain = charge(vec![member("name", Tref::Prim(Prim::String), true)]);
        let mut constrained_m = member("name", Tref::Prim(Prim::String), true);
        constrained_m.constraints = vec![Constraint::Length {
            min: None,
            max: Some(3),
        }];
        let constrained = charge(vec![constrained_m]);
        assert_eq!(
            find(
                &diff(&plain, &constrained),
                "add-constraint billing#Charge.name"
            )
            .category,
            Category::Behavioral
        );
    }

    fn enum_shape(id: &str, backing: EnumBacking, values: Vec<(String, Option<i64>)>) -> Model {
        model(vec![Shape {
            id: id.into(),
            kind: ShapeKind::Enum { backing, values },
            traits: vec![],
        }])
    }

    #[test]
    fn narrowing_an_enum_breaks_but_widening_is_safe() {
        let two = enum_shape(
            "billing#Status",
            EnumBacking::String,
            vec![("open".into(), None), ("closed".into(), None)],
        );
        let one = enum_shape(
            "billing#Status",
            EnumBacking::String,
            vec![("open".into(), None)],
        );
        assert_eq!(
            find(
                &diff(&two, &one),
                "remove-enum-value billing#Status::closed"
            )
            .category,
            Category::WireBreaking
        );
        assert_eq!(
            find(&diff(&one, &two), "add-enum-value billing#Status::closed").category,
            Category::AdditiveSafe
        );
    }

    #[test]
    fn changing_enum_backing_or_int_value_is_wire_breaking() {
        let s = enum_shape(
            "billing#Priority",
            EnumBacking::String,
            vec![("low".into(), None)],
        );
        let i = enum_shape(
            "billing#Priority",
            EnumBacking::Int,
            vec![("low".into(), Some(0))],
        );
        assert_eq!(
            find(&diff(&s, &i), "change-enum-backing billing#Priority").category,
            Category::WireBreaking
        );

        let i10 = enum_shape(
            "billing#Priority",
            EnumBacking::Int,
            vec![("low".into(), Some(10))],
        );
        assert_eq!(
            find(&diff(&i, &i10), "change-enum-value billing#Priority::low").category,
            Category::WireBreaking
        );
    }

    #[test]
    fn changing_a_union_discriminator_is_wire_breaking() {
        let union = |disc: &str| {
            model(vec![Shape {
                id: "billing#Source".into(),
                kind: ShapeKind::Union {
                    params: vec![],
                    members: vec![member(
                        "card",
                        Tref::Ref {
                            id: "billing#Card".into(),
                            args: vec![],
                        },
                        true,
                    )],
                    discriminator: disc.into(),
                },
                traits: vec![],
            }])
        };
        assert_eq!(
            find(
                &diff(&union("type"), &union("kind")),
                "change-discriminator billing#Source@discriminator"
            )
            .category,
            Category::WireBreaking
        );
    }

    #[test]
    fn removing_a_referenced_shape_is_wire_breaking_but_an_orphan_is_source_breaking() {
        let referenced = model(vec![
            structure(
                "billing#Charge",
                vec![member(
                    "card",
                    Tref::Ref {
                        id: "billing#Card".into(),
                        args: vec![],
                    },
                    true,
                )],
            ),
            structure(
                "billing#Card",
                vec![member("n", Tref::Prim(Prim::String), true)],
            ),
        ]);
        // Drop Card but keep the reference in Charge.
        let dropped = model(vec![structure(
            "billing#Charge",
            vec![member(
                "card",
                Tref::Ref {
                    id: "billing#Card".into(),
                    args: vec![],
                },
                true,
            )],
        )]);
        assert_eq!(
            find(&diff(&referenced, &dropped), "remove-shape billing#Card").category,
            Category::WireBreaking
        );

        // An orphan shape nobody references is only source-breaking.
        let orphan = model(vec![structure(
            "billing#Unused",
            vec![member("n", Tref::Prim(Prim::String), true)],
        )]);
        let empty = model(vec![]);
        assert_eq!(
            find(&diff(&orphan, &empty), "remove-shape billing#Unused").category,
            Category::SourceBreaking
        );
    }

    #[test]
    fn an_operation_signature_change_is_source_breaking() {
        let op = |input: &str| Model {
            tono_ir_version: 2,
            modules: vec![Module {
                name: "billing".into(),
                shapes: vec![],
                operations: vec![Shape {
                    id: "billing#Create".into(),
                    kind: ShapeKind::Operation {
                        input: Some(Tref::Ref {
                            id: input.into(),
                            args: vec![],
                        }),
                        output: None,
                        errors: vec![],
                    },
                    traits: vec![],
                }],
            }],
        };
        assert_eq!(
            find(
                &diff(&op("billing#A"), &op("billing#B")),
                "change-operation billing#Create$input"
            )
            .category,
            Category::SourceBreaking
        );
    }

    #[test]
    fn marking_a_shape_deprecated_is_additive() {
        let plain = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        let mut deprecated = structure(
            "billing#Charge",
            vec![member("amount", Tref::Prim(Prim::U64), true)],
        );
        deprecated.traits = vec![Trait {
            id: "core#deprecated".into(),
            value: json!("use v2"),
        }];
        assert_eq!(
            find(
                &diff(&plain, &model(vec![deprecated])),
                "add-deprecated billing#Charge"
            )
            .category,
            Category::AdditiveSafe
        );
    }

    #[test]
    fn worst_severity_applies_config_and_allowlist() {
        let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
        let after = charge(vec![member("amount", Tref::Prim(Prim::String), true)]);
        let report = diff(&before, &after);

        // Default: a wire break is an error.
        assert_eq!(report.worst(&Config::default()), Severity::Error);

        // Allowlisting that exact change key drops it to Off.
        let allowed = Config {
            allow: vec!["retype billing#Charge.amount".into()],
            ..Config::default()
        };
        assert_eq!(report.worst(&allowed), Severity::Off);

        // Configuring the whole level down to a warning also applies.
        let mut severity = BTreeMap::new();
        severity.insert(Category::WireBreaking, Severity::Warn);
        let downgraded = Config {
            severity,
            allow: vec![],
        };
        assert_eq!(report.worst(&downgraded), Severity::Warn);
    }

    #[test]
    fn category_and_severity_parse_their_cli_labels() {
        for c in [
            Category::WireBreaking,
            Category::SourceBreaking,
            Category::Behavioral,
            Category::AdditiveSafe,
        ] {
            assert_eq!(Category::parse(c.label()), Some(c));
        }
        assert_eq!(Category::parse("nonsense"), None);
        assert_eq!(Severity::parse("error"), Some(Severity::Error));
        assert_eq!(Severity::parse("warning"), Some(Severity::Warn));
        assert_eq!(Severity::parse("off"), Some(Severity::Off));
        assert_eq!(Severity::parse("loud"), None);
    }

    #[test]
    fn config_deserializes_from_json() {
        let cfg: Config = serde_json::from_str(
            r#"{"severity":{"wireBreaking":"warn","behavioral":"off"},"allow":["retype a#B.c"]}"#,
        )
        .unwrap();
        assert_eq!(
            cfg.severity.get(&Category::WireBreaking),
            Some(&Severity::Warn)
        );
        assert_eq!(
            cfg.severity.get(&Category::Behavioral),
            Some(&Severity::Off)
        );
        assert_eq!(cfg.allow, vec!["retype a#B.c".to_string()]);
    }
}

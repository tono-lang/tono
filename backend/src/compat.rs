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

use crate::codegen::taxonomy::{self, TaxonomyLiveness};
use crate::compat_entry;
use crate::compat_shape::*;
use crate::ir::{
    Constraint, EnumBacking, EnumValue, Member, Model, Module, Shape, ShapeKind, Trait, Tref,
};

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
    /// The out-of-the-box severity for a level, overridable per level in
    /// [`Config`]: a wire or source break fails the build, a behavioral change
    /// warns, and an additive change is silent.
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
            (Some(b), Some(c)) => diff_shape(b, c, &curr, &mut out),
            (None, None) => unreachable!("id came from one of the two maps"),
        }
    }
    diff_taxonomy(baseline, current, &mut out);
    Report { changes: out }
}

/// A taxonomy category's label paired with the predicate that reads its
/// liveness off a [`TaxonomyLiveness`].
type TaxonomyCategory = (&'static str, fn(&TaxonomyLiveness) -> bool);

/// The generated error-taxonomy categories a module carries, per target,
/// keyed by the label a change's detail names it with. Kept in one place so
/// this pass and its test stay in sync with what [`TaxonomyLiveness`] actually
/// tracks.
const TAXONOMY_CATEGORIES: [TaxonomyCategory; 6] = [
    ("Violation/ValidationError", |l| l.validation),
    ("TransportError", |l| l.transport),
    ("DecodeError", |l| l.decode),
    ("ContractError", |l| l.contract),
    ("APIError/APIFailure", |l| l.api),
    ("ConfigError", |l| l.config),
];

/// A target's binding-language key (as [`taxonomy::derive`] takes it) plus
/// whether it ever prunes a *loose* (non-entry) operation's taxonomy: Rust and
/// Go emit only a trait/interface for a loose operation (nothing constructs
/// any category either way, so nothing there is ever prunable), TypeScript
/// emits a concrete `HttpClient` (real, derived liveness).
const TAXONOMY_TARGETS: [(&str, &[&str], bool); 3] = [
    ("rust", &["rust"], false),
    ("go", &["go"], false),
    ("typescript", &["ts", "typescript"], true),
];

/// Detect a taxonomy category that a module could construct in the baseline
/// but can no longer construct in the current model, for a given target: a
/// consumer that referenced the removed type (e.g. caught `ContractError`
/// specifically) stops compiling, which is exactly a source break. Reuses
/// [`taxonomy::derive`] (pure over the IR, no codegen run) so this stays
/// consistent with what each target's emitter actually gates on, without
/// running codegen inside the compat tool.
///
/// A module the current model no longer has is already reported once by
/// [`removed_shape`] for its declared errors and its own removal; re-flagging
/// its taxonomy here would be a duplicate, so this only compares modules
/// present on both sides.
fn diff_taxonomy(baseline: &Model, current: &Model, out: &mut Vec<Change>) {
    let curr_modules: BTreeMap<&str, &Module> = current
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();
    for module in &baseline.modules {
        let Some(curr_module) = curr_modules.get(module.name.as_str()) else {
            continue;
        };
        for (target, langs, prunes_loose_ops) in TAXONOMY_TARGETS {
            let has_loose_ops = |m: &Module| !m.operations.is_empty();
            let liveness_of = |m: &Module| {
                if has_loose_ops(m) && !prunes_loose_ops {
                    return TaxonomyLiveness::all_live();
                }
                if target == "rust" {
                    taxonomy::derive_rust_entry(m, langs)
                } else {
                    taxonomy::derive(m, langs)
                }
            };
            let before = liveness_of(module);
            let after = liveness_of(curr_module);
            for (name, is_live) in TAXONOMY_CATEGORIES {
                if is_live(&before) && !is_live(&after) {
                    out.push(Change {
                        key: format!("prune-declaration {}@{target}#{name}", module.name),
                        category: Category::SourceBreaking,
                        detail: format!(
                            "{name} is no longer emitted for {target}: nothing in module {} can construct it anymore",
                            module.name
                        ),
                    });
                }
            }
        }
    }
}

/// Fold a model's shapes and operations into one id-keyed index. Operations live
/// in a separate vec but are themselves shapes; ids are globally unique namespaced
/// strings, so a flat cross-module index is correct.
///
/// Only what the SDK exposes is indexed. A declaration the layout routes to an
/// internal group is emitted where no consumer can name it, so it never entered
/// the compared surface and changing it cannot break anyone. Crossing the
/// boundary is still a change: a shape that leaves the surface reads as removed,
/// and one that joins it reads as added, which is exactly what they are.
fn index(model: &Model) -> BTreeMap<&str, &Shape> {
    let exposed = crate::codegen::visibility::derive(model);
    let mut m = BTreeMap::new();
    for module in &model.modules {
        for shape in module.shapes.iter().chain(module.operations.iter()) {
            if exposed.shape(shape) {
                m.insert(shape.id.as_str(), shape);
            }
        }
    }
    m
}

/// A removed shape. Still referenced from a wire position means encoded data
/// points at a type that is gone (wire break); referenced only by entry or
/// config fields (which never cross the wire), or by nothing, it is a vanished
/// declaration that breaks compiling callers (source break).
fn removed_shape(shape: &Shape, current: &BTreeMap<&str, &Shape>) -> Change {
    let on_wire = current.values().any(|s| references_on_wire(s, &shape.id));
    let category = if on_wire {
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

fn diff_shape(b: &Shape, c: &Shape, current: &BTreeMap<&str, &Shape>, out: &mut Vec<Change>) {
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
            diff_variants(&b.id, bm, cm, out);
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
        // wire is deliberately excluded from this diff: it is derived from
        // traits (@http, @timeout, @retry, ...) that this same function
        // already diffs elsewhere, so comparing it too would double-report
        // the same break under a different name.
        // input_name is deliberately excluded: the wire never carries the
        // parameter's own name, only its type/shape, so renaming it alone is
        // not a wire break.
        // impl_call is deliberately excluded too, for the same reason as
        // wire: it is bespoke implementation detail (RFC-0023's own
        // "impl .field.method(args)" op body), not part of the compared
        // wire surface.
        (
            ShapeKind::Operation {
                input: bi,
                input_name: _,
                output: bo,
                errors: be,
                wire: _,
                impl_call: _,
            },
            ShapeKind::Operation {
                input: ci,
                input_name: _,
                output: co,
                errors: ce,
                wire: _,
                impl_call: _,
            },
        ) => diff_operation(&b.id, bi, bo, be, ci, co, ce, out),
        (ShapeKind::Service { operations: bo }, ShapeKind::Service { operations: co }) => {
            diff_service(&b.id, bo, co, current, out)
        }
        // Entries and configs never cross the wire: their categories are about
        // the generated construction surface and about what resolves at
        // construction, which compat_entry separates field by field.
        (
            ShapeKind::Entry {
                fields: bf,
                operations: bo,
            },
            ShapeKind::Entry {
                fields: cf,
                operations: co,
            },
        ) => compat_entry::diff_entry(&b.id, bf, bo, cf, co, out),
        (ShapeKind::Config { fields: bf }, ShapeKind::Config { fields: cf }) => {
            compat_entry::diff_config(&b.id, bf, cf, out)
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

    if !is_deprecated(&b.traits) && is_deprecated(&c.traits) {
        out.push(deprecation_change(&path));
    }
}

/// Compare union variants by name. A union is internally tagged and closed, so a
/// removed variant can no longer decode wire data carrying its tag (wire break); an
/// added variant leaves existing data decodable (additive). A variant is not a
/// struct field, so it gets its own `*-variant` keys rather than the member ones.
fn diff_variants(id: &str, base: &[Member], curr: &[Member], out: &mut Vec<Change>) {
    let bi: BTreeMap<&str, &Member> = base.iter().map(|m| (m.name.as_str(), m)).collect();
    let ci: BTreeMap<&str, &Member> = curr.iter().map(|m| (m.name.as_str(), m)).collect();

    let mut names: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();

    for name in names {
        match (bi.get(name), ci.get(name)) {
            (Some(_), None) => out.push(Change {
                key: format!("remove-variant {id}::{name}"),
                category: Category::WireBreaking,
                detail: format!("variant {name} removed"),
            }),
            (None, Some(_)) => out.push(Change {
                key: format!("add-variant {id}::{name}"),
                category: Category::AdditiveSafe,
                detail: format!("variant {name} added"),
            }),
            (Some(b), Some(c)) => diff_variant(id, b, c, out),
            (None, None) => unreachable!("name came from one of the two maps"),
        }
    }
}

fn diff_variant(id: &str, b: &Member, c: &Member, out: &mut Vec<Change>) {
    let path = format!("{id}::{}", b.name);

    if b.target != c.target {
        out.push(Change {
            key: format!("retype-variant {path}"),
            category: Category::WireBreaking,
            detail: format!("{} -> {}", render_tref(&b.target), render_tref(&c.target)),
        });
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

    if !is_deprecated(&b.traits) && is_deprecated(&c.traits) {
        out.push(deprecation_change(&path));
    }
}

/// A new constraint, or one tightened to reject previously-valid values, changes
/// runtime behavior. Pure loosening is backward-compatible and emits nothing.
pub(crate) fn diff_constraints(
    path: &str,
    base: &[Constraint],
    curr: &[Constraint],
    out: &mut Vec<Change>,
) {
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

/// Compare enum values by tag. Every enum is open, so a removed value still
/// round-trips on the wire through the unknown arm: dropping it only removes a
/// consumer's named arm (source break), not the wire contract. An added value is
/// safe for the same reason. Changing the backing or an integer discriminant does
/// change the wire form, so those are wire breaks.
fn diff_enum(
    id: &str,
    bb: &EnumBacking,
    bv: &[EnumValue],
    cb: &EnumBacking,
    cv: &[EnumValue],
    out: &mut Vec<Change>,
) {
    if bb != cb {
        out.push(Change {
            key: format!("change-enum-backing {id}"),
            category: Category::WireBreaking,
            detail: format!("{} -> {}", backing_name(bb), backing_name(cb)),
        });
    }

    let bi: BTreeMap<&str, Option<i64>> = bv.iter().map(|v| (v.name.as_str(), v.value)).collect();
    let ci: BTreeMap<&str, Option<i64>> = cv.iter().map(|v| (v.name.as_str(), v.value)).collect();

    let mut tags: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    tags.sort_unstable();
    tags.dedup();

    for tag in tags {
        match (bi.get(tag), ci.get(tag)) {
            (Some(_), None) => out.push(Change {
                key: format!("remove-enum-value {id}::{tag}"),
                category: Category::SourceBreaking,
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
pub(crate) fn diff_operation(
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

/// A service's operation list, compared as a set. Dropping an operation the
/// service still exposes is a source break; adding one is safe. An operation that
/// was removed from the model entirely is already reported once as a removed
/// shape, so it is not double-counted here.
fn diff_service(
    id: &str,
    base: &[String],
    curr: &[String],
    current: &BTreeMap<&str, &Shape>,
    out: &mut Vec<Change>,
) {
    for op in base {
        if !curr.contains(op) && current.contains_key(op.as_str()) {
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

/// The `@wire` override on a member, or `None`. Matches the trait by its local
/// name (tolerating a `core#` namespace) and unwraps the single argument, which
/// the frontend encodes as a one-element array and the fixtures as a bare string,
/// so a `@wire` change is detected on real IR, not only on hand-authored IR.
fn wire_of(traits: &[Trait]) -> Option<&str> {
    traits
        .iter()
        .find(|t| t.id == "wire" || t.id == "core#wire")
        .and_then(|t| match &t.value {
            serde_json::Value::String(s) => Some(s.as_str()),
            serde_json::Value::Array(items) => items.first().and_then(|v| v.as_str()),
            _ => None,
        })
}

/// Whether a `@deprecated` trait is present, matching the local name so it reads
/// both the frontend's bare `deprecated` and the fixtures' `core#deprecated`.
fn is_deprecated(traits: &[Trait]) -> bool {
    traits
        .iter()
        .any(|t| t.id == "deprecated" || t.id == "core#deprecated")
}

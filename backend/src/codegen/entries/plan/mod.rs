//! The language-neutral resolution plan every entry target renders.
//!
//! The per-field construction logic (which source wins, in what order, with
//! what absence tracking, how a config composes its members) is the same shape
//! in every generated SDK: only the spelling of each leaf statement differs.
//! This module builds that shape once as a [`Stmt`] tree and walks it, asking a
//! per-target [`Emitter`] for the leaf spellings and the structural punctuation
//! its language uses. A target never re-derives the control flow; it only says
//! how an assignment, a condition, or an `if` header reads in its syntax. The
//! builders that shape the tree live in [`build`].

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::extensions::{bound_extensions, BoundExtension};
use crate::codegen::ops::op_io;
use crate::codegen::symbol::SymbolKind;
use crate::codegen::tree::Decl;
use crate::ir::{ArmValue, EntryField, EnvName, Module, Prim, Shape, Source, TemplatePart, Tref};

use super::{module_entries, source_stub, EntryModel};

mod build;
pub use build::{build_field, build_requires, presence_kind, Presence};

/// A module's entry emission, split the way the layout groups it: what every
/// entry of the module shares and, per entry, everything named after that
/// entry. Every target's own `emit` returns this same shape; only the
/// declarations inside (each target's own leaf spelling) differ.
pub struct EntryEmission {
    pub shared: Vec<Decl>,
    pub per_entry: Vec<(String, Vec<Decl>)>,
}

impl EntryEmission {
    /// What a module with no entries emits: nothing.
    pub fn empty() -> Self {
        Self {
            shared: Vec::new(),
            per_entry: Vec::new(),
        }
    }
}

/// The common setup every target's `emit` starts from: the module's entries,
/// whether it is multi-entry (so companion names need the entry's own
/// prefix), and the extensions bound for `langs`. `None` for a module with no
/// entries, so the caller returns [`EntryEmission::empty`] without building
/// anything.
pub fn entry_setup<'a>(
    module: &'a Module,
    langs: &[&str],
) -> Option<(Vec<EntryModel<'a>>, bool, Vec<BoundExtension<'a>>)> {
    let entries = module_entries(module);
    if entries.is_empty() {
        return None;
    }
    let multi = entries.len() > 1;
    let bound = bound_extensions(module, langs);
    Some((entries, multi, bound))
}

/// Render every entry field's resolution plan, in dependency order, into one
/// block of target source, each field indented `depth` levels to match the
/// scaffold it lands in (a flat function body vs. a body nested inside a
/// class's constructor).
pub fn emit_fields(
    entry: &EntryModel,
    module: &Module,
    e: &mut dyn Emitter,
    depth: usize,
) -> String {
    let mut out = String::new();
    for field in &entry.fields {
        out.push_str(&render(&build_field(field, entry, module, e), depth, e));
    }
    out
}

/// The camelCase spelling of a canonical name, for the constructor parameters
/// and reason variables that read identically in every target.
pub(crate) fn camel(name: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Camel),
        None,
    )
}

/// The camelCase constructor-parameter name of a field, honoring its
/// `@rename(lang)` override (a `@rename` retargets the whole public identifier,
/// param included, without touching the canonical value key).
pub(crate) fn arg_camel(name: &str, traits: &[crate::ir::Trait], lang: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Camel),
        crate::codegen::conventions::rename_of(traits, lang).as_deref(),
    )
}

/// The error-value variable tracking a field's deferred resolution: `nil`
/// (Go) / `undefined` (TS) while unresolved, a `ConfigError` once a source
/// step fails, cleared back to nil/undefined once a later step resolves.
pub(crate) fn err_var(field: &str) -> String {
    camel(&format!("{field}_err"))
}

/// A casing `@str::*` transform applied over `out`: the shared helper key to
/// pull in and the call spelling, identical in every target (the case-fold
/// transforms lower to the same generated `str*` helper). Returns `None` for the
/// language-specific transforms (`trim`/`lower`/`upper`) each target spells
/// itself. The key is the transform's own name.
fn casing_transform(
    t: &str,
    out: &str,
    helper: &impl Fn(&str) -> String,
) -> Option<(&'static str, String)> {
    let (key, name) = match t {
        "upper_snake" => ("upper_snake", "StrUpperSnake"),
        "snake" => ("snake", "StrSnake"),
        "kebab" => ("kebab", "StrKebab"),
        "pascal" => ("pascal", "StrPascal"),
        _ => return None,
    };
    Some((key, format!("{}({out})", helper(name))))
}

/// Fold an `@str::*` pipeline over `expr`, innermost first: `simple` spells the
/// language-specific transforms (`trim`/`lower`/`upper`) and the shared casing
/// transforms lower to the generated `Str*` helpers, recording each used key in
/// `used`. The loop is language-neutral; `simple` and `helper` (how the target
/// names a helper that lives in the SDK's shared group) differ per target.
pub(crate) fn apply_transforms(
    expr: String,
    transforms: &[String],
    used: &mut std::collections::BTreeSet<&'static str>,
    simple: impl Fn(&str, &str) -> Option<String>,
    helper: impl Fn(&str) -> String,
) -> String {
    let mut out = expr;
    for t in transforms {
        out = if let Some(s) = simple(t, &out) {
            s
        } else if let Some((key, e)) = casing_transform(t, &out, &helper) {
            used.insert(key);
            e
        } else {
            out
        };
    }
    out
}

/// A type carried as a (branded) string at rest, so its zero value is the empty
/// string and a consumed-chain require compares against `""`.
pub(crate) fn string_like(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(Prim::String | Prim::Uuid | Prim::Timestamp | Prim::Date | Prim::Duration)
            | Tref::Ref { .. }
    )
}

/// An already-spelled run of target statements, indented relative to its own
/// column zero (the walker adds the enclosing depth). No trailing newline.
pub struct Leaf(pub String);

/// An already-spelled target boolean expression (`err != nil` / `err !== undefined`).
pub struct Cond(pub String);

/// One node of the resolution plan. Straight-line code and every expression is
/// an opaque [`Leaf`] the target spells; the tree models only the branching and
/// sequencing that every target shares.
pub enum Stmt {
    Leaf(Leaf),
    Seq(Vec<Stmt>),
    /// `if c0 { .. } else if c1 { .. } .. [else { .. }]`.
    If {
        arms: Vec<(Cond, Stmt)>,
        otherwise: Option<Box<Stmt>>,
    },
    /// A brace scope with a header/footer leaf, for the config composition
    /// block (`{ var composed T; ..; dest = composed }`).
    Block {
        open: Leaf,
        body: Box<Stmt>,
        close: Leaf,
    },
    /// The native switch a `match` lowers to. `subject` is the already-spelled
    /// scrutinee; each case pairs an already-spelled pattern with its arm body.
    Switch {
        subject: String,
        cases: Vec<(String, Stmt)>,
        default: Option<Box<Stmt>>,
    },
    Nop,
}

impl Stmt {
    fn is_nop(&self) -> bool {
        matches!(self, Stmt::Nop)
    }
}

/// The per-target spelling the plan walker asks for. Every method returns an
/// already-spelled fragment; the walker owns indentation and sequencing. Import
/// and helper collection happen as side effects here, exactly as the hand
/// emitters did.
pub trait Emitter {
    /// One indentation unit (`"\t"` for Go, four spaces for TypeScript).
    fn indent_unit(&self) -> &'static str;

    /// The target's `@rename` language key (`"go"` / `"typescript"`), so a
    /// field's generated identifier honors its `@rename(lang)` override.
    fn lang(&self) -> &'static str;

    /// The `if <cond> {` / `} else if <cond> {` headers (Go omits the parens
    /// its condition needs in TypeScript, so the header is a target call).
    fn if_header(&self, cond: &Cond) -> String;

    // --- spelling atoms: the smallest per-target tokens the shared statement
    //     builders below compose. Each is a one-liner, below any clone
    //     threshold, so the composite statements live here once. ---
    /// The statement terminator (`";"` for TypeScript, empty for Go).
    fn term(&self) -> &'static str;
    /// The equality / inequality operators (`"=="`/`"!="` vs `"==="`/`"!=="`).
    fn eq(&self) -> &'static str;
    fn neq(&self) -> &'static str;
    /// The literal an error-value var reads as "not yet resolved" (`"nil"` /
    /// `"undefined"`).
    fn absent_literal(&self) -> &'static str;

    // --- the per-target primitives the delegating atoms below build on: how a
    //     field name and a sibling path spell in the target's casing, and how a
    //     literal reads in its syntax ---
    /// The settings-field read for a canonical field name (`s.Field` / `s.field`).
    fn ident(&self, name: &str) -> String;
    /// The read expression of a sibling-field path (`s.Creds.Token`).
    fn path_expr(&self, path: &[String]) -> String;
    /// The declared type at a sibling-field path.
    fn path_type(&self, path: &[String]) -> Tref;
    /// A `@default`/match-arm literal in the field's type.
    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String;
    fn member_dest(&self, member_name: &str) -> String;

    // --- delegating atoms shared across targets (the spelling lives in the
    //     primitives above and the shared naming helpers) ---
    /// The read expression of a field destination.
    fn dest(&self, field_name: &str) -> String {
        self.ident(field_name)
    }
    /// The error-value identifier for a field.
    fn err_ident(&self, field_name: &str) -> String {
        err_var(field_name)
    }
    /// The constructor parameter name of an `@arg` field, honoring its
    /// `@rename(lang)` override.
    fn arg_ident(&self, field: &EntryField) -> String {
        arg_camel(&field.name, &field.traits, self.lang())
    }
    /// The read expression of a sibling-field path (`creds.token`).
    fn path_read(&self, path: &[String]) -> String {
        self.path_expr(path)
    }
    /// The declared type at a sibling-field path.
    fn path_type_of(&self, path: &[String]) -> Tref {
        self.path_type(path)
    }
    /// Render an expression of type `t` as a target string (for env-name and
    /// error-label interpolation); pulls any import the spelling needs.
    fn to_string_expr(&mut self, expr: &str, t: &Tref) -> String;
    /// The environment read call around a name expression (`os.LookupEnv(x)` /
    /// `readEnv(x)`); records the import/helper it needs.
    fn env_read_call(&mut self, name_expr: &str) -> String;

    // --- the env lookup / label / miss reason (shared: only the read call and
    //     the to-string spelling differ) ---
    fn env_lookup(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => self.env_read_call(&format!("{n:?}")),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                let s = self.to_string_expr(&read, &t);
                self.env_read_call(&s)
            }
        }
    }
    fn env_label(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{n:?}"),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                self.to_string_expr(&read, &t)
            }
        }
    }
    /// The `ConfigError` an env lookup miss constructs: a leaf failure (no
    /// cause to wrap yet), naming the concrete variable that came up empty.
    fn env_miss_error(&mut self, name: &EnvName) -> String {
        let message = match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                let s = self.to_string_expr(&read, &t);
                format!("\"env \" + {s} + \": empty\"")
            }
        };
        self.config_error_expr(&message, None)
    }

    // --- composite statements built from the atoms (shared) ---
    fn assign_arg(&mut self, field: &EntryField, dest: &str) -> Leaf {
        Leaf(format!("{dest} = {}{}", self.arg_ident(field), self.term()))
    }
    fn assign_default(
        &mut self,
        field: &EntryField,
        value: &serde_json::Value,
        dest: &str,
    ) -> Leaf {
        Leaf(format!(
            "{dest} = {}{}",
            self.literal_of(&field.target, value),
            self.term()
        ))
    }
    /// Assign an already-spelled expression to a destination.
    fn assign_expr(&self, dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{dest} = {expr}{}", self.term()))
    }
    /// Wrap the head's error as the reason this field is still deferred: the
    /// edge being consumed wraps the cause, naming the head that failed; the
    /// head's own resolver already named the concrete source. Message
    /// composition is not uniform across targets (Go/TS concatenate with
    /// `+`; Rust has no `&str + &str` and reads the head's error through an
    /// `Option`), so this stays per-target rather than a shared default.
    fn wrap_from(&self, err_field: &str, head: &str) -> Leaf;
    /// A `ConfigError` construction expression for a leaf failure (nothing
    /// upstream to wrap yet): a raw string literal in Go, a `new
    /// ConfigError(...)` call in TS, a struct literal in Rust. `cause` is the
    /// already-spelled error value the failure wraps, when there is one.
    fn config_error_expr(&self, message_expr: &str, cause: Option<&str>) -> String;
    /// Open an error-value var (`var err error` vs `let err: ConfigError |
    /// undefined;`); the declaration keyword and type differ, so this one
    /// stays per-target. Its zero value (`nil`/`undefined`) already reads as
    /// "not yet resolved" — no sentinel value needed.
    fn err_open(&self, field_name: &str) -> Leaf;
    /// Clear an error-value var back to "resolved" (`nil`/`undefined`) once a
    /// later source in the chain succeeds.
    fn err_clear(&self, field_name: &str) -> Leaf {
        Leaf(format!(
            "{} = {}{}",
            self.err_ident(field_name),
            self.absent_literal(),
            self.term()
        ))
    }

    // --- conditions: whether a field's error var is set. Go/TS spell this
    //     with the equality atoms (`!= nil` / `!== undefined`); Rust's
    //     `Option<ConfigError>` has no `PartialEq` (its `cause` is a `Box<dyn
    //     Error>`), so it spells `.is_some()`/`.is_none()` instead — hence
    //     these stay per-target rather than a shared default. ---
    fn cond_err_present(&self, field_name: &str) -> Cond;
    fn cond_err_absent(&self, field_name: &str) -> Cond;

    // --- the source steps of a NON-guaranteed chain: each owns its guard
    //     idiom, so the sequential ordering is shared while the spelling stays
    //     per-target. `err` is the already-spelled error-value variable. ---
    fn with_step_body(&self, field: &EntryField, dest: &str, err: &str) -> String;
    fn env_step_body(
        &mut self,
        field: &EntryField,
        name: &EnvName,
        dest: &str,
        err: &str,
    ) -> String;

    // --- the whole-construct bodies that already differ per target (a
    //     guaranteed chain diverges algorithmically: Go emits an if/else-if
    //     cascade, TypeScript a set-flag sequence, so neither is shared). Each
    //     returns already-spelled statements the builders wrap into the tree. ---
    fn chain_guaranteed(&mut self, field: &EntryField, dest: &str) -> String;
    /// The `@format` assignment itself (`dest = cast(part + part + ..)` with the
    /// `@str::*` pipeline folded in); the deferral guard around it is shared.
    fn format_assign(&mut self, field: &EntryField, dest: &str) -> String;
    fn transforms_body(&mut self, field: &EntryField, dest: &str) -> Option<String>;
    fn structured_body(&mut self, field: &EntryField, shape: &Shape) -> String;
    fn json_body(&mut self, field: &EntryField) -> String;
    fn config_open(&mut self, field: &EntryField, shape: &Shape) -> Leaf;
    fn config_close(&self, dest: &str) -> Leaf {
        Leaf(format!("{dest} = composed{}", self.term()))
    }
    fn bind_expr(&self, source: &[String]) -> String {
        self.path_expr(source)
    }
    fn member_bind_assign(&self, member_dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{member_dest} = {expr}{}", self.term()))
    }

    // --- consumed-chain requires: the shared dispatch (see [`build_requires`])
    //     picks which fields need a check and which kind; each target spells the
    //     check itself (comparison, error construction, import side effect) ---
    /// A consumed member of a composed/decoded field must hold a value.
    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String;
    /// A consumed numeric/bool config member fails when its hoisted error var
    /// reports absence (a resolved zero is not absence, so the value is not
    /// consulted). `err` is the logical error-var name (see [`build`]).
    fn require_member_deferred(&mut self, name: &str, err: &str) -> String;
    /// A consumed string-like scalar must be non-empty (error-decorated).
    fn require_string(&mut self, head: &str, target: &Tref) -> String;
    /// A consumed bytes scalar must be non-empty (error-decorated).
    fn require_bytes(&mut self, head: &str) -> String;
    /// A consumed numeric scalar fails only when reported absent AND still zero.
    fn require_numeric(&mut self, head: &str, target: &Tref) -> String;

    // --- the native switch a `match` lowers to: the framing is shared (see
    //     [`build::build_select`]); only the punctuation and the unmatched-arm
    //     failure differ per target ---
    /// The switch header before its opening brace (`switch x` / `switch (x)`).
    fn switch_header(&self, subject: &str) -> String;
    /// A case / default label opening its arm (`case X:` / `case X: {`).
    fn case_open(&self, pattern: &str) -> String;
    fn default_open(&self) -> String;
    /// The break statement closing an arm body, or `None` when the language
    /// falls through by label (Go).
    fn case_tail(&self) -> Option<&'static str>;
    /// The brace closing a braced arm, or `None` when arms are label-scoped (Go).
    fn case_close(&self) -> Option<&'static str>;
    /// A match-arm pattern as a case literal.
    fn pattern_lit(&self, pattern: &serde_json::Value) -> String;
    /// The forced default arm when a match has no wildcard: a guaranteed field
    /// fails construction on an unmatched value, a deferred one records the miss.
    fn select_miss(
        &mut self,
        field: &EntryField,
        subject_head: &str,
        subject_expr: &str,
        guaranteed: bool,
    ) -> Leaf;
}

/// Render a plan into target source, each statement indented by `depth` units.
pub fn render(stmt: &Stmt, depth: usize, e: &dyn Emitter) -> String {
    let mut out = String::new();
    render_into(stmt, depth, e, &mut out);
    out
}

fn render_into(stmt: &Stmt, depth: usize, e: &dyn Emitter, out: &mut String) {
    match stmt {
        Stmt::Nop => {}
        Stmt::Leaf(Leaf(text)) => push_indented(text, depth, e.indent_unit(), out),
        Stmt::Seq(children) => {
            for child in children {
                render_into(child, depth, e, out);
            }
        }
        Stmt::If { arms, otherwise } => {
            let unit = e.indent_unit();
            let pad = unit.repeat(depth);
            for (i, (cond, body)) in arms.iter().enumerate() {
                let header = e.if_header(cond);
                if i == 0 {
                    out.push_str(&format!("{pad}{header} {{\n"));
                } else {
                    out.push_str(&format!("{pad}}} else {header} {{\n"));
                }
                render_into(body, depth + 1, e, out);
            }
            if let Some(other) = otherwise {
                out.push_str(&format!("{pad}}} else {{\n"));
                render_into(other, depth + 1, e, out);
            }
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Block { open, body, close } => {
            let unit = e.indent_unit();
            let pad = unit.repeat(depth);
            out.push_str(&format!("{pad}{{\n"));
            push_indented(&open.0, depth + 1, unit, out);
            render_into(body, depth + 1, e, out);
            push_indented(&close.0, depth + 1, unit, out);
            out.push_str(&format!("{pad}}}\n"));
        }
        Stmt::Switch {
            subject,
            cases,
            default,
        } => {
            let unit = e.indent_unit();
            let pad = unit.repeat(depth);
            out.push_str(&format!("{pad}{} {{\n", e.switch_header(subject)));
            for (pattern, body) in cases {
                render_arm(&e.case_open(pattern), body, depth, e, out);
            }
            if let Some(body) = default {
                render_arm(&e.default_open(), body, depth, e, out);
            }
            out.push_str(&format!("{pad}}}\n"));
        }
    }
}

/// One switch arm: its label at `depth`, its body one deeper, then the target's
/// optional `break` (body depth) and closing brace (label depth).
fn render_arm(label: &str, body: &Stmt, depth: usize, e: &dyn Emitter, out: &mut String) {
    let unit = e.indent_unit();
    let pad = unit.repeat(depth);
    out.push_str(&format!("{pad}{label}\n"));
    render_into(body, depth + 1, e, out);
    if let Some(tail) = e.case_tail() {
        push_indented(tail, depth + 1, unit, out);
    }
    if let Some(close) = e.case_close() {
        out.push_str(&format!("{pad}{close}\n"));
    }
}

/// Emit `text` with each non-empty line prefixed by `depth` indent units,
/// preserving the leaf's own relative structure.
fn push_indented(text: &str, depth: usize, unit: &str, out: &mut String) {
    let pad = unit.repeat(depth);
    for line in text.trim_end_matches('\n').split('\n') {
        if line.is_empty() {
            out.push('\n');
        } else {
            out.push_str(&pad);
            out.push_str(line);
            out.push('\n');
        }
    }
}

/// The bare source stub a match arm's inline sources resolve as (re-exported so
/// targets build arm chains through the same helper).
pub fn arm_sources(field: &EntryField, sources: &[Source]) -> EntryField {
    source_stub(field, sources.to_vec())
}

/// The non-guaranteed head fields a `@format` template reads, in first
/// appearance order (the template is assigned only once they resolve).
pub fn format_absent_deps(entry: &EntryModel, parts: &[TemplatePart]) -> Vec<String> {
    let mut deps: Vec<String> = Vec::new();
    for part in parts {
        if let TemplatePart::Field(p) = part {
            let head = p.first().cloned().unwrap_or_default();
            if !entry.field_guaranteed(&head) && !deps.contains(&head) {
                deps.push(head);
            }
        }
    }
    deps
}

/// The `@format` template split shared by every target that supports it:
/// each part as a target expression, plus the non-guaranteed heads it reads
/// (first-appearance order). A literal spells identically everywhere (Rust's
/// `Debug` escaping doubles as a valid Go/TypeScript string literal); only a
/// field reference needs a target-specific expression, so `field_expr` is the
/// one thing a caller supplies.
pub fn format_pieces(
    entry: &EntryModel,
    parts: &[TemplatePart],
    mut field_expr: impl FnMut(&[String]) -> String,
) -> (Vec<String>, Vec<String>) {
    let absent_deps = format_absent_deps(entry, parts);
    let mut concat: Vec<String> = Vec::new();
    for part in parts {
        match part {
            TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
            TemplatePart::Field(p) => concat.push(field_expr(p)),
            // An op-input placeholder cannot appear in a field template; the
            // frontend rejects it. Render empty defensively.
            TemplatePart::Input(_) => concat.push("\"\"".to_string()),
        }
    }
    (concat, absent_deps)
}

/// Indent every non-empty line of `s` by `n` units, relative to its own
/// column zero. Shared because every target's own version of this differed
/// only in the indent unit, already `Emitter::indent_unit`.
pub fn nest(unit: &str, s: &str, n: usize) -> String {
    let pad = unit.repeat(n);
    s.trim_end_matches('\n')
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `@arg`/error-var opening shared by the structured and whole-JSON
/// decodes: an `@arg` value passes typed (written straight onto `out`,
/// `None` returned), otherwise the error var opens and `(dest, err)` are
/// returned so the caller can lay out the `@with`/`@env`/`@default` fallback
/// cascade.
pub fn decode_opening<E: Emitter>(
    e: &mut E,
    field: &EntryField,
    out: &mut String,
) -> Option<(String, String)> {
    let dest = e.ident(&field.name);
    if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
        out.push_str(&format!("{dest} = {}{}", e.arg_ident(field), e.term()));
        return None;
    }
    let err = e.err_ident(&field.name);
    out.push_str(&e.err_open(&field.name).0);
    out.push('\n');
    Some((dest, err))
}

/// Lay out a decode field's source cascade below its error-var opening: the
/// first source runs unconditionally, every later one only while the error
/// var is still set (a first-present-wins fallback). `with_step` and
/// `env_block` are the two target-specific step bodies (a `@with` read and an
/// `@env` lookup read too differently across targets to share); `@default`
/// and the cascade's own wrapping need only primitives every `Emitter`
/// already exposes (`term`, `absent_literal`, `if_header`/`cond_err_present`),
/// so they are spelled once here.
#[allow(clippy::too_many_arguments)]
pub fn decode_cascade<E: Emitter>(
    e: &mut E,
    field: &EntryField,
    dest: &str,
    err: &str,
    mut with_step: impl FnMut(&mut E) -> String,
    mut env_block: impl FnMut(&mut E, &EnvName) -> String,
    literal: impl Fn(&Tref, &serde_json::Value) -> String,
) -> String {
    let mut steps: Vec<String> = Vec::new();
    for source in &field.sources {
        match source {
            Source::With => steps.push(with_step(e)),
            Source::Env(name) => steps.push(env_block(e, name)),
            Source::Default(v) => steps.push(format!(
                "{dest} = {}{t}\n{err} = {absent}{t}",
                literal(&field.target, v),
                t = e.term(),
                absent = e.absent_literal(),
            )),
            Source::Arg => {}
        }
    }
    steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            if i == 0 {
                step.clone()
            } else {
                format!(
                    "{} {{\n{}\n}}",
                    e.if_header(&e.cond_err_present(&field.name)),
                    nest(e.indent_unit(), step, 1)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `ident` occurs in `body` and every occurrence is a write, never a
/// read: a declaration (`is_decl` sees the trimmed text immediately before
/// the identifier) or a plain reassignment (`=`, not `==`; `extra_write` is
/// an additional token that also counts as a write when it immediately
/// follows — Go's `:=` short declaration, which Rust has no equivalent of).
/// Shared because both Go and Rust discard an error var nothing consumes
/// (their compilers reject a write-only local); TypeScript needs no such
/// pass, since an unused `let` is not an error there.
pub fn is_written_never_read(
    body: &str,
    ident: &str,
    is_decl: impl Fn(&str) -> bool,
    extra_write: Option<&str>,
) -> bool {
    let bytes = body.as_bytes();
    let mut found = false;
    let mut from = 0;
    while let Some(rel) = body[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        from = end;
        let whole_word = (start == 0 || !is_ident_byte(bytes[start - 1]))
            && (end == bytes.len() || !is_ident_byte(bytes[end]));
        if !whole_word {
            continue;
        }
        found = true;
        let decl = is_decl(body[..start].trim_end());
        let rest = body[end..].trim_start_matches(' ');
        let is_write = decl
            || extra_write.is_some_and(|w| rest.starts_with(w))
            || (rest.starts_with('=') && !rest.starts_with("=="));
        if !is_write {
            return false;
        }
    }
    found
}

/// The per-type output-decode helpers every operation that decodes a body
/// needs (a protocol response or a raw bespoke outcome), deduped by shape so
/// operations sharing an output type share one decode helper instead of each
/// repeating its required-member probe. `decodes_body` is the one predicate
/// that differs per target (Go/TypeScript also route a raw bespoke impl
/// through this path; Rust does not yet); `per_shape` is the per-target decode
/// declarations for one shape.
pub fn output_decode_decls(
    entries: &[EntryModel<'_>],
    module: &Module,
    decodes_body: impl Fn(&Shape) -> bool,
    mut per_shape: impl FnMut(&Shape) -> Option<Decl>,
) -> Vec<Decl> {
    let mut seen = std::collections::BTreeSet::new();
    let mut decls = Vec::new();
    for entry in entries {
        for op in entry.operations {
            if !decodes_body(op) {
                continue;
            }
            let (_, output) = op_io(op);
            let Some(Tref::Ref { id, .. }) = output else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                decls.extend(per_shape(shape));
            }
        }
    }
    decls
}

/// The head field of an inline match-arm source path or a select subject.
pub fn arm_value_head(value: &ArmValue) -> Option<String> {
    match value {
        ArmValue::Field(p) => p.first().cloned(),
        _ => None,
    }
}

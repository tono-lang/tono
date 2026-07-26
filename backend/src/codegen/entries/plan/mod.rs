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
use crate::codegen::symbol::SymbolKind;
use crate::ir::{ArmValue, EntryField, EnvName, Module, Prim, Shape, Source, TemplatePart, Tref};

use super::{source_stub, EntryModel};

mod build;
pub use build::{build_field, build_requires, env_name_prereq, presence_kind, Presence};

/// Render every entry field's resolution plan, in dependency order, into one
/// block of target source (each field indented one level).
pub fn emit_fields(entry: &EntryModel, module: &Module, e: &mut dyn Emitter) -> String {
    let mut out = String::new();
    for field in &entry.fields {
        out.push_str(&render(&build_field(field, entry, module, e), 1, e));
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

/// The reason ("why") variable tracking a field's deferred resolution.
pub(crate) fn why_var(field: &str) -> String {
    camel(&format!("{field}_why"))
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

/// An already-spelled target boolean expression (`why != ""` / `why !== ""`).
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
    /// The why-var identifier for a field.
    fn why_ident(&self, field_name: &str) -> String {
        why_var(field_name)
    }
    /// The constructor parameter name of an `@arg` field.
    fn arg_ident(&self, field: &EntryField) -> String {
        camel(&field.name)
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
    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type_of(&fr.field);
                let read = self.path_read(&fr.field);
                let s = self.to_string_expr(&read, &t);
                format!("\"env \" + {s} + \": empty\"")
            }
        }
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
    /// Record that a field's value is still deferred to the head it reads
    /// (`why = "head <- " + headWhy`).
    fn assign_reason(&self, why_field: &str, head: &str) -> Leaf {
        Leaf(format!(
            "{} = \"{head} <- \" + {}{}",
            self.why_ident(why_field),
            self.why_ident(head),
            self.term()
        ))
    }
    /// Open a why-var (`why := "x"` vs `let why = "x";`); the declaration
    /// keyword differs, so this one stays per-target.
    fn why_open(&self, field_name: &str, initial: &str) -> Leaf;
    fn why_set(&self, why_field: &str, reason: &str) -> Leaf {
        Leaf(format!(
            "{} = {reason:?}{}",
            self.why_ident(why_field),
            self.term()
        ))
    }

    // --- conditions (shared, from the operator atoms) ---
    fn cond_why_absent(&self, field_name: &str) -> Cond {
        Cond(format!(
            "{} {} \"\"",
            self.why_ident(field_name),
            self.neq()
        ))
    }
    fn cond_why_resolved(&self, field_name: &str) -> Cond {
        Cond(format!("{} {} \"\"", self.why_ident(field_name), self.eq()))
    }

    // --- the source steps of a NON-guaranteed chain: each owns its guard
    //     idiom, so the sequential ordering is shared while the spelling stays
    //     per-target. `why` is the already-spelled reason variable. ---
    fn with_step_body(&self, field: &EntryField, dest: &str, why: &str) -> String;
    fn env_step_body(
        &mut self,
        field: &EntryField,
        name: &EnvName,
        dest: &str,
        why: &str,
    ) -> String;
    /// The opened guard an env step chains onto when the variable's own name is
    /// a still-deferred sibling `head` (`if headWhy != "" { .. } else `); the
    /// scaffold that decides when it is needed lives in [`env_name_prereq`].
    fn name_prereq_line(&self, head: &str, why: &str) -> String;

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
    fn member_chain_body(&mut self, stub: &EntryField, dest: &str) -> String;

    // --- consumed-chain requires: the shared dispatch (see [`build_requires`])
    //     picks which fields need a check and which kind; each target spells the
    //     check itself (comparison, error construction, import side effect) ---
    /// A consumed member of a composed/decoded field must hold a value.
    fn require_member(&mut self, head: &str, member: &str, leaf: &Tref, name: &str) -> String;
    /// A consumed string-like scalar must be non-empty (why-decorated error).
    fn require_string(&mut self, head: &str, target: &Tref) -> String;
    /// A consumed bytes scalar must be non-empty (why-decorated error).
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

/// The head field of an inline match-arm source path or a select subject.
pub fn arm_value_head(value: &ArmValue) -> Option<String> {
    match value {
        ArmValue::Field(p) => p.first().cloned(),
        _ => None,
    }
}

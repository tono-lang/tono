//! The language-neutral resolution plan every entry target renders.
//!
//! The per-field construction logic (which source wins, in what order, with
//! what absence tracking, how a config composes its members) is the same shape
//! in every generated SDK: only the spelling of each leaf statement differs.
//! This module builds that shape once as a [`Stmt`] tree and walks it, asking a
//! per-target [`Emitter`] for the leaf spellings and the structural punctuation
//! its language uses. A target never re-derives the control flow; it only says
//! how an assignment, a condition, or an `if` header reads in its syntax
//! (RFC-0006: emission through a component tree, never parallel string
//! concatenation).

use crate::ir::{ArmValue, EntryField, Module, Shape, ShapeKind, Source, TemplatePart, Tref};

use super::{source_stub, EntryModel, FieldShape};

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
    /// The read expression of a field / member destination.
    fn dest(&self, field_name: &str) -> String;
    fn member_dest(&self, member_name: &str) -> String;
    /// The why-var identifier for a field.
    fn why_ident(&self, field_name: &str) -> String;
    /// The constructor parameter name of an `@arg` field.
    fn arg_ident(&self, field: &EntryField) -> String;
    /// A `@default`/match-arm literal in the field's type.
    fn literal_of(&self, target: &Tref, value: &serde_json::Value) -> String;

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
    //     per-target ---
    fn with_step(&mut self, field: &EntryField, dest: &str, why_field: &str) -> Leaf;
    fn env_step(
        &mut self,
        field: &EntryField,
        source: &Source,
        dest: &str,
        why_field: &str,
    ) -> Leaf;

    // --- the whole-construct leaves that already differ per target (a
    //     guaranteed chain diverges algorithmically: Go emits an if/else-if
    //     cascade, TypeScript a set-flag sequence, so neither is shared) ---
    fn chain_guaranteed_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf;
    fn select_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf;
    fn format_leaf(&mut self, field: &EntryField, dest: &str) -> Leaf;
    fn transforms_leaf(&mut self, field: &EntryField, dest: &str) -> Option<Leaf>;
    fn structured_leaf(&mut self, field: &EntryField, shape: &Shape) -> Stmt;
    fn json_leaf(&mut self, field: &EntryField) -> Stmt;
    fn config_open(&mut self, field: &EntryField, shape: &Shape) -> Leaf;
    fn config_close(&self, dest: &str) -> Leaf {
        Leaf(format!("{dest} = composed{}", self.term()))
    }
    fn bind_expr(&self, source: &[String]) -> String;
    fn member_bind_assign(&self, member_dest: &str, expr: &str) -> Leaf {
        Leaf(format!("{member_dest} = {expr}{}", self.term()))
    }
    fn member_select_leaf(&mut self, member: &EntryField, dest: &str) -> Leaf;
    fn member_format_leaf(&mut self, member: &EntryField, dest: &str) -> Leaf;
    fn member_chain(&mut self, member: &EntryField, sources: &[Source], dest: &str) -> Stmt;
    fn member_transforms_leaf(&mut self, member: &EntryField, dest: &str) -> Option<Leaf>;
}

fn has_arg(field: &EntryField) -> bool {
    field.sources.iter().any(|s| matches!(s, Source::Arg))
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
        FieldShape::Structured(shape) => e.structured_leaf(field, shape),
        FieldShape::Json => e.json_leaf(field),
        FieldShape::Scalar => build_scalar(field, entry, e),
    }
}

/// A scalar field: an `@arg`, a match, a `@format` derivation, or a plain
/// chain, then the declared `@str::*` pipeline over whatever it produced
/// (`@format` folds the pipeline into its own template).
fn build_scalar(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter) -> Stmt {
    let dest = e.dest(&field.name);
    if field.format.is_some() {
        return Stmt::Leaf(e.format_leaf(field, &dest));
    }
    let head = if has_arg(field) {
        Stmt::Leaf(e.assign_arg(field, &dest))
    } else if field.select.is_some() {
        Stmt::Leaf(e.select_leaf(field, &dest))
    } else {
        build_chain(field, entry, e, &dest)
    };
    seq(vec![head, opt_leaf(e.transforms_leaf(field, &dest))])
}

/// The plain source chain of one field. A guaranteed chain is a per-target
/// leaf (Go and TypeScript spell it with different algorithms). A
/// non-guaranteed one opens a why-var and tries each source in turn, sharing
/// the sequential-fallback shape.
fn build_chain(field: &EntryField, entry: &EntryModel, e: &mut dyn Emitter, dest: &str) -> Stmt {
    if entry.is_guaranteed(field) {
        Stmt::Leaf(e.chain_guaranteed_leaf(field, dest))
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
            Source::With => Stmt::Leaf(e.with_step(field, dest, why)),
            Source::Env(_) => Stmt::Leaf(e.env_step(field, source, dest, why)),
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
                        otherwise: Some(Box::new(build_member(member, e, &member_dest))),
                    }
                }
            }
            None => build_member(member, e, &member_dest),
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
fn build_member(member: &EntryField, e: &mut dyn Emitter, dest: &str) -> Stmt {
    let head = if member.select.is_some() {
        Stmt::Leaf(e.member_select_leaf(member, dest))
    } else if member.format.is_some() {
        Stmt::Leaf(e.member_format_leaf(member, dest))
    } else {
        e.member_chain(member, &member.sources, dest)
    };
    if member.format.is_some() {
        head
    } else {
        seq(vec![head, opt_leaf(e.member_transforms_leaf(member, dest))])
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

//! The shared helpers every target's `resolve.rs`/`mod.rs` calls instead of
//! duplicating the same control flow with only its own leaf spellings
//! swapped in. Split out of `mod.rs` to stay under this repo's per-file line
//! ceiling; nothing here builds the [`super::Stmt`] tree itself (that stays
//! in `mod.rs`/`build.rs`), it only composes [`Emitter`] calls.

use crate::codegen::ops::op_io;
use crate::codegen::tree::Decl;
use crate::ir::{ArmValue, EntryField, EnvName, Module, Shape, Source, Tref};

use super::{Emitter, EntryModel};

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

/// The four pieces every `@env` presence step reads before spelling its own
/// syntax around them: the read expression, the interpolatable label, the
/// miss-error expression, and the sibling-name prereq guard.
pub fn env_parts<E: Emitter>(
    e: &mut E,
    name: &EnvName,
    err: &str,
) -> (String, String, String, String) {
    let lookup = e.env_lookup(name);
    let label = e.env_label(name);
    let miss = e.env_miss_error(name);
    let pre = e.env_name_prereq(name, err);
    (lookup, label, miss, pre)
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

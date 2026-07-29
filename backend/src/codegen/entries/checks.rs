//! The value-path checks shared across every target's entry resolution: the
//! presence-guard decision, and (for the targets whose settings receiver is
//! a bare `s` — Rust and TypeScript; Go's own construction shape does not
//! use this) the field-access and runtime-value-freeze spellings. Split out
//! of `entries/mod.rs` to stay under this repo's per-file line ceiling.

use super::{EntryModel, ValuePath};
use crate::codegen::casing::{transform, CasingConfig};
use crate::codegen::conventions::rename_of;
use crate::codegen::entries::plan;
use crate::codegen::symbol::SymbolKind;
use crate::ir::Tref;

/// Whether a frozen value entry needs a presence guard before it is written to
/// the runtime values. A guaranteed non-member field is always present, and a
/// non-string value freezes unconditionally (its zero reads the same as its
/// absence to the runtime's value positions). Shared so each target spells only
/// the comparison operator, not the decision.
pub fn needs_presence_guard(entry: &EntryModel, vp: &ValuePath) -> bool {
    if entry.is_guaranteed(vp.field) && vp.member.is_none() {
        return false;
    }
    plan::string_like(vp.target)
}

/// The settings-field read for a value path's own field, honoring its
/// `@rename(lang)` override; a member path tail is plain (unrenamed). Shared
/// across the targets whose settings receiver is a bare `s` (Rust and
/// TypeScript; Go's own construction shape does not use this), since the
/// casing transform itself is already language-neutral — only the target's
/// own `CasingConfig` (passed in) and `lang` key vary.
pub fn value_path_access(vp: &ValuePath<'_>, config: &CasingConfig, lang: &str) -> String {
    let head = transform(
        &vp.field.name,
        SymbolKind::Field,
        config,
        rename_of(&vp.field.traits, lang).as_deref(),
    );
    match &vp.member {
        None => head,
        Some(member) => format!(
            "{head}.{}",
            transform(member, SymbolKind::Field, config, None)
        ),
    }
}

/// Whether a value path freezes into the runtime values, and if so, the
/// (already-idiomatic) expression reading it off `s`. A named reference
/// freezes when it resolves to a scalar (an enum is a branded string on the
/// wire), which is what `scalar_ref` says; a map/list never freezes as a
/// single value. Shares [`value_path_access`]'s scope (Rust/TypeScript).
pub fn value_path_frozen_expr(
    vp: &ValuePath<'_>,
    config: &CasingConfig,
    lang: &str,
    scalar_ref: bool,
) -> Option<String> {
    if matches!(vp.target, Tref::Ref { .. }) && !scalar_ref {
        return None;
    }
    if matches!(vp.target, Tref::Map(_, _) | Tref::List(_)) {
        return None;
    }
    Some(format!("s.{}", value_path_access(vp, config, lang)))
}

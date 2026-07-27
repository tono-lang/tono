//! Reading the core trait vocabulary off the IR.
//!
//! A trait is written one way in source and reaches the generator in two: the
//! frontend emits the id bare (`wire`) and encodes a single positional argument
//! as a one-element array, while hand-authored IR and the golden fixtures spell
//! the id with its namespace (`core#wire`) and the argument bare. Every reader
//! has to accept both, and the whole vocabulary lives here so no reader can be
//! written that accepts only one.

use crate::codegen::tree::TypeExpr;
use crate::ir::{Member, Trait};

/// A trait from the core vocabulary, by its local name.
///
/// The frontend emits a trait id bare (`wire`); the golden fixtures and
/// hand-authored IR spell it with its namespace (`core#wire`). Both are the same
/// trait, so every reader matches on the local name and tolerates the namespace.
/// This is the single place that knows the rule: a reader that spelled its own
/// comparison could match only the namespaced form, which never reaches it from
/// a real project, and the trait would be silently ignored.
pub fn core_trait<'a>(traits: &'a [Trait], name: &str) -> Option<&'a Trait> {
    traits
        .iter()
        .find(|t| t.id == name || t.id.strip_prefix("core#") == Some(name))
}

/// A trait's single argument as a string.
///
/// The frontend encodes one positional argument as a one-element array
/// (`["use v2"]`); hand-authored IR writes the bare string. A trait with no
/// argument yields `None`.
fn string_arg(t: &Trait) -> Option<String> {
    match &t.value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            items.first().and_then(|v| v.as_str()).map(str::to_string)
        }
        _ => None,
    }
}

/// The `@rename(lang)` identifier override (a value object keyed by language).
/// Replaces the in-code identifier only; never the wire key.
pub fn rename_of(traits: &[Trait], lang: &str) -> Option<String> {
    rename_map(traits)
        .and_then(|t| t.value.get(lang))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The `@rename` trait itself, for callers that need every override rather than
/// one language's.
pub fn rename_map(traits: &[Trait]) -> Option<&Trait> {
    core_trait(traits, "rename")
}

/// The `@wire` serialization-key override. Replaces the wire key only; never the
/// in-code identifier.
pub fn wire_of(traits: &[Trait]) -> Option<String> {
    core_trait(traits, "wire").and_then(string_arg)
}

/// The serialization key for a member: its `@wire` override, else the canonical
/// name. Independent of the in-code identifier.
pub fn wire_key(member: &Member) -> String {
    wire_of(&member.traits).unwrap_or_else(|| member.name.clone())
}

/// Whether a member carries the `@entries` map-escape trait.
pub fn has_entries(traits: &[Trait]) -> bool {
    core_trait(traits, "entries").is_some()
}

/// The `@deprecated` reason, or `None` when absent. A bare `@deprecated` (no
/// argument) yields `Some("")`. Every target rebases this onto its native
/// deprecation annotation, keeping the element generated but marked.
pub fn deprecated_of(traits: &[Trait]) -> Option<String> {
    core_trait(traits, "deprecated").map(|t| string_arg(t).unwrap_or_default())
}

/// The `@doc` Markdown content, or `None` when absent or empty. A documentation
/// comment with no content is not worth emitting. Every target lowers this onto
/// its native doc format (JSDoc, rustdoc, godoc).
pub fn doc_of(traits: &[Trait]) -> Option<String> {
    core_trait(traits, "doc")
        .and_then(string_arg)
        .filter(|s| !s.is_empty())
}

/// Reshape a map into an `@entries` pairs-array when the member carries the
/// `core#entries` trait; any other type is unchanged. The escape only applies to
/// a map (a non-map `@entries` is rejected upstream by the typechecker).
pub fn entries_or_map(ty: TypeExpr, traits: &[Trait]) -> TypeExpr {
    match ty {
        TypeExpr::Map(key, value) if has_entries(traits) => TypeExpr::Entries(key, value),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn trait_of(id: &str, value: serde_json::Value) -> Trait {
        Trait {
            id: id.into(),
            value,
        }
    }

    /// The core trait vocabulary, spelled the way the frontend emits it: the id
    /// bare, a single positional argument as a one-element array, a bare trait
    /// as `null`. Hand-authored IR writes the namespaced id and the bare value,
    /// so every reader has to accept both.
    ///
    /// This is a contract test, not a unit test: a reader that matches only the
    /// namespaced form compiles, passes every fixture written that way, and then
    /// silently ignores the trait for every real project. `@rename`, `@wire`, and
    /// `@entries` each shipped with exactly that defect.
    #[test]
    fn every_core_trait_reader_accepts_the_id_the_frontend_emits() {
        let frontend = |id: &str, value: serde_json::Value| vec![trait_of(id, value)];
        let authored =
            |id: &str, value: serde_json::Value| vec![trait_of(&format!("core#{id}"), value)];

        for build in [frontend, authored] {
            assert_eq!(
                doc_of(&build("doc", json!(["text"]))).as_deref(),
                Some("text")
            );
            assert_eq!(
                doc_of(&build("doc", json!("text"))).as_deref(),
                Some("text")
            );
            assert_eq!(
                deprecated_of(&build("deprecated", json!(["use v2"]))).as_deref(),
                Some("use v2")
            );
            // A bare @deprecated carries no argument and still marks the element.
            assert_eq!(
                deprecated_of(&build("deprecated", json!(null))).as_deref(),
                Some("")
            );
            assert_eq!(
                wire_of(&build("wire", json!(["userId"]))).as_deref(),
                Some("userId")
            );
            assert_eq!(
                wire_of(&build("wire", json!("userId"))).as_deref(),
                Some("userId")
            );
            assert!(has_entries(&build("entries", json!(null))));
            assert_eq!(
                rename_of(&build("rename", json!({"go": "Modified"})), "go").as_deref(),
                Some("Modified")
            );
        }

        // A trait the vocabulary does not name is not matched by a suffix.
        assert!(doc_of(&[trait_of("lab#doc", json!(["text"]))]).is_none());
        assert!(wire_of(&[]).is_none());
        assert!(!has_entries(&[]));
    }
}

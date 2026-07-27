//! Emission groups: the unit of output the whole engine is laid out around.
//!
//! A group answers two questions about a batch of declarations: where its name
//! comes from (a declaration in the spec, or the generator itself) and who it is
//! emitted for (the SDK's consumers, or only the SDK). Every target maps a group
//! onto whatever it has that expresses those two things: a Go package under
//! `internal/`, a private Rust `mod`, a TypeScript subpath left out of the
//! package's `exports`.
//!
//! A group belongs either to an IR module or to the SDK root. The root group
//! holds what crosses module boundaries (the branded well-known types, the
//! serde runtime helpers), so a two-module SDK carries one copy rather than one
//! per module; everything that serves a single module lives in that module's
//! groups.
//!
//! Groups are referenced by a *path*: a single string, since the component tree
//! and the import engine key on module paths and a group is what a module path
//! now denotes. [`Group::path`] and [`parse_path`] are the only places that
//! encoding is spelled out.

use std::collections::HashMap;

/// Where a group's name comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Origin {
    /// The name is a declaration's own: an entry named `admin` produces a group
    /// named `admin`, so the SDK's layout reads like the spec.
    Spec,
    /// A fixed name the generator owns, the same in every SDK.
    Generator,
}

/// Who a group's declarations are emitted for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Audience {
    /// Part of the SDK's surface: a consumer may name it.
    Public,
    /// Reachable only from inside the SDK. Each target marks it with whatever it
    /// has to keep a consumer out.
    Internal,
}

/// The generator-owned group holding a module's public types and its error
/// surface.
pub const TYPES: &str = "types";
/// The generator-owned group holding a module's serialization.
pub const CODEC: &str = "codec";
/// The generator-owned internal group, both at the SDK root and per module.
pub const INTERNAL: &str = "internal";

/// The SDK-root internal group's path, spelled out for the emitters that
/// reference a shared helper from raw text and need to name where it lives.
pub const ROOT: &str = "::internal";

/// The separator between a group's module and its name in a group path. Two
/// colons cannot occur in a dotted module name, so a path parses unambiguously
/// and a plain module name is never mistaken for one.
const SEP: &str = "::";

/// One emission group.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Group {
    /// The IR module this group belongs to, or `None` for the SDK-root group.
    pub module: Option<String>,
    pub name: String,
    pub origin: Origin,
    pub audience: Audience,
}

impl Group {
    /// The SDK-root internal group: what crosses module boundaries and no
    /// consumer names.
    pub fn root_internal() -> Self {
        Self {
            module: None,
            name: INTERNAL.into(),
            origin: Origin::Generator,
            audience: Audience::Internal,
        }
    }

    /// A module's public group: its exported types and error surface.
    pub fn types(module: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: TYPES.into(),
            origin: Origin::Generator,
            audience: Audience::Public,
        }
    }

    /// A module's codec group: the serialization of the types it exposes.
    ///
    /// Internal, but it cannot be moved away from them: a method has to be
    /// declared where its receiver is (Go), and an impl where its type is
    /// (Rust). So this group sits beside the module rather than under the SDK's
    /// internal tree, and each target hides it in place.
    pub fn codec(module: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: CODEC.into(),
            origin: Origin::Generator,
            audience: Audience::Internal,
        }
    }

    /// A module's internal group: the declarations no public type reaches, and
    /// their serialization. Nothing outside the SDK names them, so this group is
    /// free to move where the target fences it off.
    pub fn module_internal(module: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: INTERNAL.into(),
            origin: Origin::Generator,
            audience: Audience::Internal,
        }
    }

    /// Whether this group has to stay beside the module it belongs to, because
    /// its declarations extend the module's own types.
    pub fn is_colocated(&self) -> bool {
        self.name == CODEC
    }

    /// The group of one entry, named after the entry declaration. Two entries in
    /// a module therefore produce two groups.
    pub fn entry(module: &str, entry: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: entry.into(),
            origin: Origin::Spec,
            audience: Audience::Public,
        }
    }

    /// This group's path, the string the component tree and the import engine
    /// carry in place of a bare module name.
    pub fn path(&self) -> String {
        format!("{}{SEP}{}", self.module.as_deref().unwrap_or(""), self.name)
    }

    pub fn is_internal(&self) -> bool {
        self.audience == Audience::Internal
    }
}

/// Split a group path back into its module (`None` at the SDK root) and group
/// name. Returns `None` for a string that is not a group path, which is how a
/// plain module name or an external package specifier is told apart.
pub fn parse_path(path: &str) -> Option<(Option<&str>, &str)> {
    let (module, name) = path.split_once(SEP)?;
    Some((Some(module).filter(|m| !m.is_empty()), name))
}

/// The IR module a group path belongs to, or `None` at the SDK root (and for
/// anything that is not a group path).
pub fn module_of(path: &str) -> Option<&str> {
    parse_path(path).and_then(|(module, _)| module)
}

/// Which group defines each symbol.
///
/// Emitters build symbols against the IR module a type belongs to, because that
/// is all a Symbol table knows; the group that ends up holding the declaration is
/// decided later, when the module is split. This index is the record of that
/// decision, filled by one pass over the emitted files, and it is what lets
/// import collection re-point a reference at the file that actually declares it
/// instead of at the module as a whole.
#[derive(Debug, Default, Clone)]
pub struct SymbolIndex {
    by_symbol: HashMap<(String, String), String>,
    default_group: HashMap<String, String>,
}

impl SymbolIndex {
    /// Where a symbol of `module` resolves when no group claimed it by name.
    ///
    /// A declaration emitted as opaque text (a Go union's interface and its
    /// variant wrappers) has no name the tree can be read for, so it would
    /// otherwise leave a reference pointing at a bare module name, which no
    /// target can spell. The module's public group is the right answer: opaque
    /// or not, a name another module references is part of the surface.
    pub fn set_default(&mut self, module: &str, group: &str) {
        self.default_group
            .insert(module.to_string(), group.to_string());
    }

    /// Record that `group` (a group path) declares `symbol` for IR `module`.
    /// The first registration wins, so a symbol a target repeats across files
    /// (a Go union's marker methods, say) resolves to where it is defined.
    pub fn insert(&mut self, module: &str, symbol: &str, group: &str) {
        self.by_symbol
            .entry((module.to_string(), symbol.to_string()))
            .or_insert_with(|| group.to_string());
    }

    /// The group path declaring `symbol` in `module`, or `None` when the symbol
    /// is not the SDK's (a standard-library or runtime-package import).
    pub fn group_of(&self, module: &str, symbol: &str) -> Option<&str> {
        self.by_symbol
            .get(&(module.to_string(), symbol.to_string()))
            .or_else(|| self.default_group.get(module))
            .map(String::as_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_group_path_round_trips() {
        let group = Group::types("payments.common");
        assert_eq!(group.path(), "payments.common::types");
        assert_eq!(
            parse_path(&group.path()),
            Some((Some("payments.common"), "types"))
        );
        assert_eq!(module_of(&group.path()), Some("payments.common"));
    }

    #[test]
    fn the_root_group_carries_no_module() {
        let group = Group::root_internal();
        assert_eq!(group.path(), ROOT);
        assert_eq!(parse_path(&group.path()), Some((None, "internal")));
        assert_eq!(module_of(&group.path()), None);
        assert!(group.is_internal());
    }

    #[test]
    fn a_plain_module_name_is_not_a_group_path() {
        // Import collection tells an unresolved module name and an external
        // package specifier from a group path by exactly this.
        assert_eq!(parse_path("payments.common"), None);
        assert_eq!(parse_path("encoding/json"), None);
        assert_eq!(parse_path("@tono/http-runtime-ts"), None);
    }

    #[test]
    fn an_entry_group_is_named_after_its_declaration() {
        let group = Group::entry("notes", "admin");
        assert_eq!(group.name, "admin");
        assert_eq!(group.origin, Origin::Spec);
        assert_eq!(group.audience, Audience::Public);
        assert_eq!(group.path(), "notes::admin");
        // Two entries in one module are two distinct groups.
        assert_ne!(Group::entry("notes", "client").path(), group.path());
    }

    #[test]
    fn generator_groups_carry_their_audience() {
        assert!(!Group::types("notes").is_internal());
        assert!(Group::module_internal("notes").is_internal());
        assert_eq!(Group::types("notes").origin, Origin::Generator);
    }

    #[test]
    fn the_symbol_index_resolves_a_symbol_to_its_defining_group() {
        let mut index = SymbolIndex::default();
        index.insert("payments.common", "Card", "payments.common::types");
        index.insert(
            "payments.common",
            "unmarshalMethod",
            "payments.common::internal",
        );
        assert_eq!(
            index.group_of("payments.common", "Card"),
            Some("payments.common::types")
        );
        assert_eq!(
            index.group_of("payments.common", "unmarshalMethod"),
            Some("payments.common::internal")
        );
        // A symbol the SDK does not declare stays unresolved.
        assert_eq!(index.group_of("encoding/json", "json"), None);
        assert_eq!(index.group_of("payments.charges", "Card"), None);
    }

    #[test]
    fn the_first_registration_of_a_symbol_wins() {
        let mut index = SymbolIndex::default();
        index.insert("notes", "Note", "notes::types");
        index.insert("notes", "Note", "notes::internal");
        assert_eq!(index.group_of("notes", "Note"), Some("notes::types"));
    }
}

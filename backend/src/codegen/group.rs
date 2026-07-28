//! Emission groups: the unit of output the whole engine is laid out around.
//!
//! A group answers two questions about a batch of declarations: where its name
//! comes from (a declaration in the spec, or the generator itself) and who it is
//! emitted for (the SDK's consumers, or only the SDK). Every target maps a group
//! onto whatever it has that expresses those two things: a Go package under
//! `internal/`, a private Rust `mod`, a TypeScript subpath left out of the
//! package's `exports`.
//!
//! A group belongs either to an IR module or to the SDK root. The root groups
//! hold what crosses module boundaries (the branded well-known types, the
//! serialization, the resolution of declared construction values), so a
//! two-module SDK carries one copy rather than one per module; everything that
//! serves a single module lives in that module's groups.
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
/// The generator-owned internal group of a module.
pub const INTERNAL: &str = "internal";

/// The SDK-root group holding the serialization every module shares.
pub const ROOT_CODEC_NAME: &str = CODEC;

/// The SDK-root group holding the resolution of declared construction values
/// (an environment variable, a duration, a casing transform), which every
/// module's entry clients share.
pub const ROOT_CONFIG_NAME: &str = "config";

/// The generator-owned group holding what every module of the SDK shares and a
/// consumer names: the branded well-known types.
pub const SUPPORT: &str = "support";

/// The SDK-root groups' paths, spelled out for the emitters that reference a
/// shared helper from raw text and need to name where it lives.
///
/// There are two rather than one because a single name for both would be a name
/// for neither: an SDK reader meeting `codec` and `config` knows what each holds,
/// and one named for the generator tells them nothing.
pub const ROOT_CODEC: &str = "::codec";
pub const ROOT_CONFIG: &str = "::config";

/// The SDK-root support group's path. A symbol table names it when it maps a
/// well-known primitive, since those are one set of types for the whole SDK
/// rather than one per module: two modules' `Timestamp` have to be the same
/// type, or a value of one cannot be handed to the other.
pub const ROOT_SUPPORT: &str = "::support";

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
    /// Whether the group has to stay beside the module it belongs to. Internal
    /// and yet unmovable: its declarations extend the module's own types, and a
    /// method is declared where its receiver is (Go), an impl where its type is
    /// (Rust). Every other internal group is free to move where the target
    /// fences it off.
    pub colocated: bool,
}

impl Group {
    /// The SDK-root serialization group: the encoding every module shares.
    pub fn root_codec() -> Self {
        Self::root(ROOT_CODEC_NAME)
    }

    /// The SDK-root configuration group: resolving the declared construction
    /// values every module's entry clients read.
    pub fn root_config() -> Self {
        Self::root(ROOT_CONFIG_NAME)
    }

    fn root(name: &str) -> Self {
        Self {
            module: None,
            name: name.into(),
            origin: Origin::Generator,
            audience: Audience::Internal,
            colocated: false,
        }
    }

    /// The SDK-root support group: the declarations every module shares and a
    /// consumer names, so they are one set of types rather than one per module.
    pub fn root_support() -> Self {
        Self {
            module: None,
            name: SUPPORT.into(),
            origin: Origin::Generator,
            audience: Audience::Public,
            colocated: false,
        }
    }

    /// A module's public group: its exported types and error surface.
    pub fn types(module: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: TYPES.into(),
            origin: Origin::Generator,
            audience: Audience::Public,
            colocated: false,
        }
    }

    /// A module's codec group: the serialization of the types it exposes.
    ///
    /// Internal, but it cannot be moved away from them: a method has to be
    /// declared where its receiver is (Go), and an impl where its type is
    /// (Rust). So this group stays beside the module wherever the target could
    /// otherwise have moved it, and each target hides it in place.
    pub fn codec(module: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: CODEC.into(),
            origin: Origin::Generator,
            audience: Audience::Internal,
            colocated: true,
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
            colocated: false,
        }
    }

    /// The group of one entry, named after the entry declaration. Two entries in
    /// a module therefore produce two groups.
    pub fn entry(module: &str, entry: &str) -> Self {
        Self {
            module: Some(module.into()),
            name: entry.into(),
            origin: Origin::Spec,
            audience: Audience::Public,
            colocated: false,
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

    /// The group a path names, or `None` for a string that is not a group path.
    ///
    /// A path carries only the module and the name, but every generator-owned
    /// name has a fixed audience and any other name is an entry's (spec-named
    /// and public), so the rest of the group is recoverable. This is what lets
    /// the layout answer a reference (which arrives as a path) with the same
    /// rules it placed the file by.
    pub fn from_path(path: &str) -> Option<Self> {
        let (module, name) = parse_path(path)?;
        Some(match (module, name) {
            (None, ROOT_CODEC_NAME) => Self::root_codec(),
            (None, ROOT_CONFIG_NAME) => Self::root_config(),
            (None, SUPPORT) => Self::root_support(),
            (None, _) => return None,
            (Some(module), TYPES) => Self::types(module),
            (Some(module), CODEC) => Self::codec(module),
            (Some(module), INTERNAL) => Self::module_internal(module),
            (Some(module), entry) => Self::entry(module, entry),
        })
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
    /// Nested rather than keyed on a pair, so a lookup borrows both halves
    /// instead of allocating a key for every reference the engine resolves.
    by_symbol: HashMap<String, HashMap<String, String>>,
    default_group: HashMap<String, String>,
}

impl SymbolIndex {
    /// Where a symbol of `module` resolves when no group claimed it by name.
    ///
    /// The last resort, not the normal path: an emitter that declares a name
    /// through opaque text lists it (`provides`), so the index knows it exactly.
    /// This catches a name no emitter listed, and answers with the module's
    /// public group, which is the only answer that cannot make the output worse:
    /// a name another module references is part of the surface, and pointing at
    /// a bare module name would leave a path no target can spell.
    pub fn set_default(&mut self, module: &str, group: &str) {
        self.default_group
            .insert(module.to_string(), group.to_string());
    }

    /// Record that `group` (a group path) declares `symbol` for IR `module`.
    /// The first registration wins, so a symbol a target repeats across files
    /// (a Go union's marker methods, say) resolves to where it is defined.
    pub fn insert(&mut self, module: &str, symbol: &str, group: &str) {
        self.by_symbol
            .entry(module.to_string())
            .or_default()
            .entry(symbol.to_string())
            .or_insert_with(|| group.to_string());
    }

    /// The group path declaring `symbol` in `module`, or `None` when the symbol
    /// is not the SDK's (a standard-library or runtime-package import).
    pub fn group_of(&self, module: &str, symbol: &str) -> Option<&str> {
        self.by_symbol
            .get(module)
            .and_then(|names| names.get(symbol))
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
    fn every_group_is_recoverable_from_its_path() {
        // A reference arrives as a path, so the layout has to answer it with the
        // same group it placed the file by.
        for group in [
            Group::root_codec(),
            Group::root_support(),
            Group::types("payments.common"),
            Group::codec("payments.common"),
            Group::module_internal("payments.common"),
            Group::entry("payments.charges", "client"),
        ] {
            assert_eq!(Group::from_path(&group.path()).as_ref(), Some(&group));
        }
        // Anything that is not a group path has no group.
        assert_eq!(Group::from_path("payments.common"), None);
        assert_eq!(Group::from_path("encoding/json"), None);
        assert_eq!(Group::from_path("::client"), None);
    }

    #[test]
    fn the_root_support_group_is_public_and_generator_owned() {
        let group = Group::root_support();
        assert_eq!(group.path(), ROOT_SUPPORT);
        assert!(!group.is_internal());
        assert_eq!(group.origin, Origin::Generator);
    }

    #[test]
    fn the_root_groups_carry_no_module_and_are_named_for_their_contents() {
        for (group, path, name) in [
            (Group::root_codec(), ROOT_CODEC, ROOT_CODEC_NAME),
            (Group::root_config(), ROOT_CONFIG, ROOT_CONFIG_NAME),
        ] {
            assert_eq!(group.path(), path);
            assert_eq!(parse_path(&group.path()), Some((None, name)));
            assert_eq!(module_of(&group.path()), None);
            assert!(group.is_internal());
        }
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
    fn generator_groups_carry_their_audience_and_whether_they_can_move() {
        assert!(!Group::types("notes").is_internal());
        assert!(Group::module_internal("notes").is_internal());
        assert_eq!(Group::types("notes").origin, Origin::Generator);
        // Both are internal; only the codec group is pinned beside its module,
        // because its declarations extend that module's own types.
        assert!(Group::codec("notes").is_internal());
        assert!(Group::codec("notes").colocated);
        assert!(!Group::module_internal("notes").colocated);
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

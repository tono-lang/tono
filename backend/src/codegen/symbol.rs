//! The Symbol abstraction: the per-language representation of an IR type.
//!
//! A Symbol is the load-bearing piece of the typed codegen engine. The
//! component tree manipulates Symbols, not text, so referencing a Symbol
//! anywhere in the tree contributes its import to the enclosing file
//! automatically (transitively, through `references`), and the idiomatic name is
//! emitted by construction. Targets build Symbols; the engine collects imports.

/// Where a referenced Symbol is imported from. The target decides how this
/// renders into a concrete import statement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Import {
    /// Module / package / crate path, interpreted by the target.
    pub module: String,
    /// The name as it appears in the import statement, which may differ from the
    /// symbol's in-code `name` (e.g. an aliased or default import).
    pub imported: String,
}

/// The kind of a symbol, so the casing transform can pick the idiomatic default
/// casing for the position the symbol appears in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Type,
    Field,
    Method,
    EnumMember,
    Variant,
    Module,
}

/// The per-language representation of an IR type: an idiomatic, already-cased
/// `name`, an optional `import` collected automatically when the symbol is
/// referenced, and the `references` it transitively depends on (generic
/// arguments, field types) which drive transitive import collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    pub import: Option<Import>,
    pub references: Vec<Symbol>,
}

impl Symbol {
    /// A built-in / primitive symbol: a name with no import and no references
    /// (e.g. `string`, `i64`). Nothing to import; nothing to recurse into.
    pub fn builtin(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            import: None,
            references: Vec::new(),
        }
    }

    /// A symbol brought in from another module: its in-code `name` plus the
    /// `import` (module path + imported name) collected when it is referenced.
    pub fn imported(
        name: impl Into<String>,
        module: impl Into<String>,
        imported: impl Into<String>,
    ) -> Self {
        Self {
            name: name.into(),
            import: Some(Import {
                module: module.into(),
                imported: imported.into(),
            }),
            references: Vec::new(),
        }
    }

    /// Attach the transitive references this symbol depends on (generic
    /// arguments, field types). These carry imports the enclosing file must also
    /// collect, so a `Page<Charge>` pulls both `Page` and `Charge`.
    #[must_use]
    pub fn referencing(mut self, references: Vec<Symbol>) -> Self {
        self.references = references;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_has_no_import_and_no_references() {
        let s = Symbol::builtin("string");
        assert_eq!(s.name, "string");
        assert_eq!(s.import, None);
        assert!(s.references.is_empty());
    }

    #[test]
    fn imported_carries_its_import() {
        let s = Symbol::imported("Charge", "payments", "Charge");
        assert_eq!(s.name, "Charge");
        assert_eq!(
            s.import,
            Some(Import {
                module: "payments".into(),
                imported: "Charge".into(),
            })
        );
        assert!(s.references.is_empty());
    }

    #[test]
    fn referencing_attaches_transitive_dependencies() {
        let page = Symbol::imported("Page", "core", "Page")
            .referencing(vec![Symbol::imported("Charge", "payments", "Charge")]);
        assert_eq!(page.references.len(), 1);
        assert_eq!(page.references[0].name, "Charge");
    }

    #[test]
    fn imported_name_may_differ_from_in_code_name() {
        // A default or aliased import: in-code `Client`, imported as `default`.
        let s = Symbol::imported("Client", "@sdk/core", "default");
        let import = s.import.expect("an import");
        assert_ne!(import.imported, s.name);
        assert_eq!(import.imported, "default");
    }

    #[test]
    fn symbol_kinds_are_distinct() {
        assert_ne!(SymbolKind::Type, SymbolKind::Field);
        // Copy is available for cheap passing to the casing transform.
        let k = SymbolKind::Method;
        let copy = k;
        assert_eq!(k, copy);
    }
}

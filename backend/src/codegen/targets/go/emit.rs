//! Assembling a Go module from an IR module into separate output files so the types
//! can be read without the serialization noise: a types file (the branded well-known
//! named strings, the `Entry`/`Entries` definitions when `@entries` is used, and
//! each shape's type declarations — structs, enums, and a union's interface,
//! wrappers, and markers) and, when there is anything to serialize, a serde file
//! (`marshalVariant`, the `Entries` (de)serialization methods, each union's wrapper
//! `MarshalJSON`s and `unmarshalX`, and each container's `UnmarshalJSON`). Imports
//! are derived per file from the symbols its declarations reference, so the types
//! file pulls nothing while the serde file pulls `encoding/json` (plus `fmt` for a
//! union); a module of plain tagged structs emits only the types file.
//!
//! The Go package clause is not part of a rendered file (the engine emits imports
//! first); the caller prepends `package <name>` before formatting, once per file.
//! See [`package_clause`].

use std::collections::HashSet;

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::{has_entries, type_ident};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::{
    emit_serde_decls, runtime_serde_helpers, runtime_type_helpers, RuntimeHelpers,
};
use crate::codegen::targets::go::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File, ModuleFile};
use crate::ir::{Module, ShapeKind};

/// The Go language key for per-language traits such as `@rename`.
const LANG: &str = "go";

/// The branded well-known types: distinct named string types, so they serialize
/// exactly as their inner value while staying distinct in code.
pub fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration"]
        .iter()
        .map(|name| {
            Decl::Alias(Alias {
                name: Symbol::builtin(*name),
                value: "string".into(),
            })
        })
        .collect()
}

/// The Go package clause for a module name, which the caller prepends before
/// formatting (the rendered file starts with imports, so the clause cannot be a
/// declaration).
pub fn package_clause(name: &str) -> String {
    format!("package {name}\n")
}

/// The identifiers of every union shape in the module, used to detect a struct
/// field whose type is a union (which then needs a container `UnmarshalJSON`).
fn union_idents(module: &Module) -> HashSet<String> {
    module
        .shapes
        .iter()
        .filter(|s| matches!(s.kind, ShapeKind::Union { .. }))
        .map(|s| type_ident(s, LANG))
        .collect()
}

/// Whether any structure member in the module carries the `@entries` escape, which
/// is the only thing that pulls the generic `Entries[K, V]` helper.
fn uses_entries(module: &Module) -> bool {
    module.shapes.iter().any(|s| match &s.kind {
        ShapeKind::Structure { members, .. } => members.iter().any(|m| has_entries(&m.traits)),
        _ => false,
    })
}

/// Whether the module has any union, which pulls the `marshalVariant` helper.
fn uses_union(module: &Module) -> bool {
    module
        .shapes
        .iter()
        .any(|s| matches!(s.kind, ShapeKind::Union { .. }))
}

/// Assemble a Go module into separate output files: the types file (well-known
/// named strings, the `Entry`/`Entries` definitions when `@entries` is used, and
/// each shape's type declarations) and, when there is any serialization to emit,
/// the serde file (`marshalVariant`, the `Entries` (de)serialization methods, each
/// union's wrapper `MarshalJSON`s and `unmarshalX`, and each container's
/// `UnmarshalJSON`). A module of plain tagged structs emits only the types file:
/// `encoding/json` does all its work, so there is nothing for the serde file to
/// hold. Imports are derived per file from the symbols its declarations reference,
/// so the types file pulls nothing while the serde file pulls `encoding/json`
/// (plus `fmt` when a union is present).
pub fn emit_module(module: &Module, config: &CasingConfig) -> Vec<ModuleFile> {
    let unions = union_idents(module);
    let helpers = RuntimeHelpers {
        entries: uses_entries(module),
        variant: uses_union(module),
    };

    let mut type_decls = well_known_decls();
    type_decls.extend(runtime_type_helpers(helpers));
    let mut serde_decls = runtime_serde_helpers(helpers);
    for shape in &module.shapes {
        type_decls.extend(emit_type(shape, config));
        serde_decls.extend(emit_serde_decls(shape, config, &unions));
    }

    let mut files = vec![ModuleFile {
        suffix: "",
        file: File {
            module: module.name.clone(),
            decls: type_decls,
        },
    }];
    // A pure-types module (no union, no @entries, no union-bearing container) emits
    // no serde file at all.
    if !serde_decls.is_empty() {
        files.push(ModuleFile {
            suffix: "_serde",
            file: File {
                module: module.name.clone(),
                decls: serde_decls,
            },
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::render::render_file;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{member, structure, union_shape};
    use crate::codegen::Formatter;
    use crate::ir::{Prim, Tref};

    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    /// Render the text of the file with the given basename suffix ("" types,
    /// "_serde" serialization), panicking if the module did not emit it.
    fn rendered(files: &[ModuleFile], suffix: &str) -> String {
        let mf = files
            .iter()
            .find(|f| f.suffix == suffix)
            .unwrap_or_else(|| panic!("module did not emit a {suffix:?} file"));
        render_file(&mf.file, &GoRules, &passthrough()).text
    }

    #[test]
    fn the_package_clause_names_the_module() {
        assert_eq!(package_clause("models"), "package models\n");
    }

    #[test]
    fn a_module_of_plain_structs_emits_only_a_types_file_with_no_imports() {
        let module = Module {
            name: "models".into(),
            shapes: vec![structure(
                "models#Charge",
                vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
        };
        let files = emit_module(&module, &go_casing());
        // A pure-types module emits a single file: there is no serialization to hold.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].suffix, "");
        let out = rendered(&files, "");
        // Well-known named strings and the tagged type are present; with no union and
        // no @entries, no runtime helper and no import is emitted.
        assert!(out.contains("type Timestamp string"));
        assert!(out.contains("type Charge struct {"));
        // The type holds the 64-bit integer natively, tagged `,string`.
        assert!(out.contains("\tAmountCents int64 `json:\"amount_cents,string\"`\n"));
        assert!(!out.contains("func marshalVariant("));
        assert!(!out.contains("type Entries["));
        assert!(!out.contains("import "));
    }

    #[test]
    fn a_module_with_a_union_splits_types_from_serde() {
        let module = Module {
            name: "models".into(),
            shapes: vec![
                structure(
                    "models#Account",
                    vec![member(
                        "method",
                        Tref::Ref {
                            id: "models#Method".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                union_shape(
                    "models#Method",
                    "type",
                    vec![member(
                        "card",
                        Tref::Ref {
                            id: "models#card_data".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                structure(
                    "models#card_data",
                    vec![member("last4", Tref::Prim(Prim::String), true)],
                ),
            ],
            operations: vec![],
        };
        let files = emit_module(&module, &go_casing());
        assert_eq!(files.len(), 2);

        // The types file holds the interface, wrappers, markers, and the struct types,
        // with no serialization and no import at all.
        let types = rendered(&files, "");
        assert!(types.contains("type Method interface{ isMethod() }"));
        assert!(types.contains("type MethodCard struct{ Value CardData }"));
        assert!(types.contains("func (MethodCard) isMethod() {}"));
        assert!(types.contains("type Account struct {"));
        assert!(types.contains("type CardData struct {"));
        assert!(!types.contains("import "));
        assert!(!types.contains("MarshalJSON"));
        assert!(!types.contains("UnmarshalJSON"));
        assert!(!types.contains("func marshalVariant("));

        // The serde file holds marshalVariant, the wrapper MarshalJSON, the dispatcher,
        // and the container UnmarshalJSON; it pulls encoding/json and fmt, but never
        // imports the module itself (the payload type is same-package).
        let serde = rendered(&files, "_serde");
        assert!(serde.contains("func marshalVariant("));
        assert!(serde.contains(
            "func (m MethodCard) MarshalJSON() ([]byte, error) { return marshalVariant("
        ));
        assert!(serde.contains("func unmarshalMethod(b []byte) (Method, error) {"));
        assert!(serde.contains("func (a *Account) UnmarshalJSON(b []byte) error {"));
        assert!(serde.contains("import \"encoding/json\""));
        assert!(serde.contains("import \"fmt\""));
        assert!(!serde.contains("import \"models\""));
        // The interface and wrapper definitions stay out of the serde file.
        assert!(!serde.contains("type Method interface"));
        assert!(!serde.contains("type MethodCard struct"));
    }

    #[test]
    fn a_module_with_an_entries_field_splits_the_definition_from_its_methods() {
        let mut counts = member(
            "counts",
            Tref::Map(
                Box::new(Tref::Prim(Prim::I32)),
                Box::new(Tref::Prim(Prim::String)),
            ),
            true,
        );
        counts.traits = vec![crate::ir::Trait {
            id: "core#entries".into(),
            value: serde_json::json!(true),
        }];
        let module = Module {
            name: "models".into(),
            shapes: vec![structure("models#Doc", vec![counts])],
            operations: vec![],
        };
        let files = emit_module(&module, &go_casing());
        assert_eq!(files.len(), 2);

        // The Entry/Entries definitions and the typed field live in the types file,
        // with no imports and no (de)serialization methods.
        let types = rendered(&files, "");
        assert!(types.contains("type Entry[K comparable, V any] struct {"));
        assert!(types.contains("type Entries[K comparable, V any] []Entry[K, V]"));
        assert!(types.contains("\tCounts Entries[int32, string] `json:\"counts\"`\n"));
        assert!(!types.contains("import "));
        assert!(!types.contains("MarshalJSON"));

        // The Entries methods live in the serde file, which pulls encoding/json; with
        // no union there is no marshalVariant.
        let serde = rendered(&files, "_serde");
        assert!(serde.contains("func (e Entries[K, V]) MarshalJSON() ([]byte, error) {"));
        assert!(serde.contains("func (e *Entries[K, V]) UnmarshalJSON(b []byte) error {"));
        assert!(serde.contains("import \"encoding/json\""));
        assert!(!serde.contains("func marshalVariant("));
    }
}

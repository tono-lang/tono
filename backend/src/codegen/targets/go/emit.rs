//! Assembling a whole Go module from an IR module: the branded well-known named
//! string types and only the shared runtime helpers the module actually uses,
//! then each shape's type declaration plus any codec it needs (a union's interface
//! and dispatcher, or a container `UnmarshalJSON`). Imports are derived by the
//! engine at render time from the symbols the declarations reference, so a module
//! of plain tagged structs pulls no imports at all.
//!
//! The Go package clause is not part of the rendered file (the engine emits
//! imports first); the caller prepends `package <name>` before formatting. See
//! [`package_clause`].

use std::collections::HashSet;

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::{has_entries, type_ident};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::{emit_codecs, runtime_helpers, RuntimeHelpers};
use crate::codegen::targets::go::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File};
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

/// Assemble a complete Go module file for an IR module.
pub fn emit_module(module: &Module, config: &CasingConfig) -> File {
    let unions = union_idents(module);
    let mut decls = well_known_decls();
    decls.extend(runtime_helpers(RuntimeHelpers {
        entries: uses_entries(module),
        variant: uses_union(module),
    }));
    for shape in &module.shapes {
        decls.extend(emit_type(shape, config));
        decls.extend(emit_codecs(shape, config, &unions));
    }
    File {
        module: module.name.clone(),
        decls,
    }
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

    #[test]
    fn the_package_clause_names_the_module() {
        assert_eq!(package_clause("models"), "package models\n");
    }

    #[test]
    fn a_module_of_plain_structs_emits_tagged_types_and_no_helpers_or_imports() {
        let module = Module {
            name: "models".into(),
            shapes: vec![structure(
                "models#Charge",
                vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
        };
        let out = render_file(
            &emit_module(&module, &go_casing()),
            &GoRules,
            &passthrough(),
        )
        .text;
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
    fn a_module_with_a_union_emits_the_interface_dispatcher_and_json_imports() {
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
        let out = render_file(
            &emit_module(&module, &go_casing()),
            &GoRules,
            &passthrough(),
        )
        .text;
        // The interface, the dispatcher, marshalVariant, and the container method are
        // emitted; the codecs pull encoding/json and fmt; the same-module payload
        // pulls no import.
        assert!(out.contains("type Method interface{ isMethod() }"));
        assert!(out.contains("func unmarshalMethod(b []byte) (Method, error) {"));
        assert!(out.contains("func marshalVariant("));
        assert!(out.contains("func (a *Account) UnmarshalJSON(b []byte) error {"));
        assert!(out.contains("import \"encoding/json\""));
        assert!(out.contains("import \"fmt\""));
        assert!(!out.contains("import \"models\""));
        assert!(out.contains("type CardData struct {"));
    }

    #[test]
    fn a_module_with_an_entries_field_uses_the_entries_type() {
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
        let out = render_file(
            &emit_module(&module, &go_casing()),
            &GoRules,
            &passthrough(),
        )
        .text;
        // The generic Entries helper is emitted (entries are used); the field is
        // typed as it, with a plain tag. No marshalVariant: there is no union.
        assert!(out.contains("type Entries[K comparable, V any] []Entry[K, V]"));
        assert!(out.contains("\tCounts Entries[int32, string] `json:\"counts\"`\n"));
        assert!(out.contains("import \"encoding/json\""));
        assert!(!out.contains("func marshalVariant("));
    }
}

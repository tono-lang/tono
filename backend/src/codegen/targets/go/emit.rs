//! Assembling a whole Go module from an IR module: the branded well-known named
//! string types and the shared codec runtime helpers once, then each shape's
//! clean type declaration plus its codecs (the union's sealed interface rides
//! along with its codecs). Imports are derived by the engine at render time from
//! the symbols the declarations reference.
//!
//! The Go package clause is not part of the rendered file (the engine emits
//! imports first); the caller prepends `package <name>` before formatting. See
//! [`package_clause`].

use crate::codegen::casing::CasingConfig;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::{emit_codecs, runtime_helpers};
use crate::codegen::targets::go::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File};
use crate::ir::Module;

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

/// Assemble a complete Go module file for an IR module.
pub fn emit_module(module: &Module, config: &CasingConfig) -> File {
    let mut decls = well_known_decls();
    decls.extend(runtime_helpers());
    for shape in &module.shapes {
        decls.extend(emit_type(shape, config));
        decls.extend(emit_codecs(shape, config));
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
    fn a_module_emits_well_known_helpers_clean_types_and_codecs() {
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
        // Well-known named strings, the codec runtime helpers, a clean type, and the
        // per-shape codecs are all present and ordered.
        assert!(out.contains("type Timestamp string"));
        assert!(out.contains("func encodeI64(v int64) any"));
        assert!(out.contains("type Charge struct {"));
        // The clean type holds the 64-bit integer natively, no json tag.
        assert!(out.contains("\tAmountCents int64\n"));
        assert!(!out.contains("json:"));
        assert!(out.contains("func encodeCharge(v Charge) any {"));
        assert!(out.contains("m[\"amount_cents\"] = encodeI64(v.AmountCents)"));
    }

    #[test]
    fn a_module_with_a_union_emits_the_sealed_interface_and_collects_stdlib_imports() {
        let module = Module {
            name: "models".into(),
            shapes: vec![
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
        // The sealed interface and codecs are emitted; the runtime helpers pull the
        // stdlib imports; the same-module payload pulls no import.
        assert!(out.contains("type Method interface{ isMethod() }"));
        assert!(out.contains("func encodeMethod(v Method) any {"));
        assert!(out.contains("import \"strconv\""));
        assert!(out.contains("import \"encoding/base64\""));
        assert!(out.contains("import \"fmt\""));
        assert!(!out.contains("import \"models\""));
        assert!(out.contains("type CardData struct {"));
    }

    #[test]
    fn a_module_with_an_entries_field_codes_it_as_a_pairs_array() {
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
        // The generic Entry helper is always emitted; the field is a slice of it and
        // its codec walks the pairs.
        assert!(out.contains("type Entry[K any, V any] struct {"));
        assert!(out.contains("\tCounts []Entry[int32, string]\n"));
        assert!(out.contains("m[\"counts\"] = encodeEntries(v.Counts,"));
    }
}

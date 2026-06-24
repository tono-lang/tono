//! Assembling a whole Go module from an IR module: the branded well-known named
//! string types, the shared `marshalTagged` helper (only when the module has a
//! union), then each shape's declaration (a struct, the named-string enum, or the
//! verbatim union item). Imports are derived by the engine at render time from
//! the symbols the declarations reference.
//!
//! The Go package clause is not part of the rendered file (the engine emits
//! imports first); the caller prepends `package <name>` before formatting. See
//! [`package_clause`].

use crate::codegen::casing::CasingConfig;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::marshal_tagged_helper;
use crate::codegen::targets::go::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File};
use crate::ir::{Module, ShapeKind};

/// The branded well-known types: distinct named string types, so they serialize
/// exactly as their inner value while staying distinct in code.
pub fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration", "Uuid"]
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
    // The marshalTagged helper is only needed when some shape is a union.
    if module
        .shapes
        .iter()
        .any(|s| matches!(s.kind, ShapeKind::Union { .. }))
    {
        decls.push(marshal_tagged_helper());
    }
    for shape in &module.shapes {
        decls.extend(emit_type(shape, config));
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
    use crate::codegen::Formatter;
    use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    fn member(name: &str, target: Tref, required: bool) -> Member {
        Member {
            name: name.into(),
            target,
            required,
            default: None,
            constraints: vec![],
            traits: vec![],
        }
    }

    fn structure(id: &str, members: Vec<Member>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members,
            },
            traits: vec![],
        }
    }

    #[test]
    fn the_package_clause_names_the_module() {
        assert_eq!(package_clause("models"), "package models\n");
    }

    #[test]
    fn a_module_without_unions_omits_the_marshal_helper() {
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
        // Well-known named strings are present; the union helper is not.
        assert!(out.contains("type Timestamp string"));
        assert!(!out.contains("func marshalTagged"));
        assert!(out.contains("type Charge struct {"));
        assert!(out.contains("\tAmountCents int64 `json:\"amount_cents,string\"`"));
    }

    #[test]
    fn a_module_with_a_union_emits_the_helper_and_collects_stdlib_imports() {
        let module = Module {
            name: "models".into(),
            shapes: vec![
                Shape {
                    id: "models#Method".into(),
                    kind: ShapeKind::Union {
                        params: vec![],
                        discriminator: "type".into(),
                        members: vec![member(
                            "card",
                            Tref::Ref {
                                id: "models#CardData".into(),
                                args: vec![],
                            },
                            true,
                        )],
                    },
                    traits: vec![],
                },
                structure(
                    "models#CardData",
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
        // The helper is emitted; the union's methods pull the stdlib imports; the
        // same-module payload pulls no import.
        assert!(out.contains("func marshalTagged"));
        assert!(out.contains("import \"encoding/json\""));
        assert!(out.contains("import \"fmt\""));
        assert!(!out.contains("import \"models\""));
        assert!(out.contains("type Method struct {"));
        assert!(out.contains("type CardData struct {"));
    }
}

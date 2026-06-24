//! Building the component tree from IR shapes for the Go target: `emit_type` plus
//! the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become structs and enums become named-string types (their consts
//! are a render concern). Unions have no Go sum type, so they are emitted as a
//! struct with one pointer per variant plus custom JSON methods by a later phase.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, type_ident, type_name};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::union_item;
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The Go language key for per-language traits such as `@rename`.
const LANG: &str = "go";

/// The idiomatic Go casing: exported PascalCase fields (so `encoding/json` can
/// see them). Type names are PascalCase in the IR and used as-is; enum values are
/// wire strings kept verbatim, so only the field default matters here.
pub fn go_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Pascal)
}

/// Convert an IR type reference into a component-tree type expression through the
/// Go symbol table.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    conventions::type_expr_of(t, &symbol_of)
}

/// Emit the declaration(s) for a shape. Structures become struct interfaces;
/// enums become named-string enum decls; unions are emitted by a later phase.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, LANG),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => vec![Decl::Enum(EnumDecl {
            name: type_name(shape, LANG),
            // Enum values are wire strings kept verbatim; the const identifiers
            // are derived at render time.
            members: values
                .iter()
                .map(|(value, _)| Symbol::builtin(value.clone()))
                .collect(),
        })],
        ShapeKind::Union {
            discriminator,
            members,
            ..
        } => vec![union_item(discriminator, members, &type_ident(shape, LANG))],
        _ => vec![],
    }
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        // Go always carries the wire key in the json tag.
        wire: Some(conventions::wire_key(member)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{member, structure};
    use crate::ir::Prim;

    #[test]
    fn a_structure_becomes_a_struct_with_exported_fields_and_wire_keys() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("account_id", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &go_casing());
        // Exported PascalCase identifiers (with acronym), the wire key carried on
        // every field, and nullability preserved.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "AccountID"
                && i.fields[0].wire.as_deref() == Some("account_id")
                && !i.fields[0].nullable
                && i.fields[1].name.name == "Note"
                && i.fields[1].nullable));
    }

    #[test]
    fn an_enum_becomes_a_named_string_with_verbatim_values() {
        let shape = Shape {
            id: "billing#Status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("pending".into(), None), ("settled".into(), None)],
            },
            traits: vec![],
        };
        let decls = emit_type(&shape, &go_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"));
    }

    #[test]
    fn a_union_becomes_a_verbatim_pointer_struct_item() {
        let union = Shape {
            id: "billing#Method".into(),
            kind: ShapeKind::Union {
                params: vec![],
                discriminator: "type".into(),
                members: vec![member(
                    "card",
                    Tref::Ref {
                        id: "billing#CardData".into(),
                        args: vec![],
                    },
                    true,
                )],
            },
            traits: vec![],
        };
        let decls = emit_type(&union, &go_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("type Method struct {")
                && r.text.contains("MarshalJSON")
                && r.text.contains("\tCard *CardData")));
    }

    #[test]
    fn services_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &go_casing()).is_empty());
    }
}

//! Building the component tree from IR shapes for the Go target: `emit_type` plus
//! the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become structs and enums become named-string types (their consts
//! are a render concern). Unions have no Go sum type, so they are emitted as a
//! struct with one pointer per variant plus custom JSON methods by a later phase.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::union_item;
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr};
use crate::ir::{Member, Shape, Tref};

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
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // A Go enum is a named string built from its wire literals; the const
        // identifiers are derived at render time.
        |values, name| vec![conventions::string_enum(values, name)],
        |discriminator, members, name| vec![union_item(discriminator, members, name)],
    )
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
    use crate::codegen::test_support::{enum_shape, member, structure, union_shape};
    use crate::ir::{Prim, ShapeKind};

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
        let shape = enum_shape(
            "billing#Status",
            vec![("pending".into(), None), ("settled".into(), None)],
        );
        let decls = emit_type(&shape, &go_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"));
    }

    #[test]
    fn a_union_becomes_a_verbatim_pointer_struct_item() {
        let union = union_shape(
            "billing#Method",
            "type",
            vec![member(
                "card",
                Tref::Ref {
                    id: "billing#card_data".into(),
                    args: vec![],
                },
                true,
            )],
        );
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

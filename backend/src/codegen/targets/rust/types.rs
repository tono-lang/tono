//! Building the component tree from IR shapes for the Rust target: `emit_type`
//! plus the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become `struct` interfaces here. Enums and unions need custom
//! serde plumbing (a custom `Deserialize` for the open `Unknown` arm, a tagged
//! `enum` for unions) and are emitted as verbatim items by a later phase.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, type_ident, type_name};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::rust::codecs::{enum_item, union_item};
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, Interface, TypeExpr};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The Rust language key for per-language traits such as `@rename`.
const LANG: &str = "rust";

/// The idiomatic Rust casing: snake_case fields. Type names are PascalCase in the
/// IR and used as-is (not cased); only the field default matters here.
pub fn rust_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Snake)
}

/// Convert an IR type reference into a component-tree type expression through the
/// Rust symbol table.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    conventions::type_expr_of(t, &symbol_of)
}

/// Emit the declaration(s) for a shape. Structures become struct interfaces;
/// enums and unions become verbatim items (custom serde impls) built by the
/// codec layer. Other shape kinds contribute nothing here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, LANG),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => {
            vec![enum_item(values, &type_ident(shape, LANG))]
        }
        ShapeKind::Union {
            discriminator,
            members,
            ..
        } => vec![union_item(discriminator, members, &type_ident(shape, LANG))],
        _ => vec![],
    }
}

/// The PascalCase Rust identifier for an open-enum or union variant, derived from
/// its wire value / member name. Independent of the wire string the codec emits.
pub(crate) fn variant_ident(name: &str, config: &CasingConfig) -> String {
    casing::transform(name, SymbolKind::Variant, config, None)
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        wire: conventions::wire_of(&member.traits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{member, structure};
    use crate::ir::Prim;

    #[test]
    fn a_structure_becomes_a_struct_with_snake_fields() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "amount_cents"
                && !i.fields[0].nullable
                && i.fields[1].name.name == "note"
                && i.fields[1].nullable));
    }

    #[test]
    fn an_enum_becomes_a_verbatim_open_enum_item() {
        let shape = Shape {
            id: "billing#Status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("pending".into(), None)],
            },
            traits: vec![],
        };
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("pub enum Status {") && r.text.contains("Unknown(String)")));
    }

    #[test]
    fn a_union_becomes_a_verbatim_tagged_enum_item() {
        let shape = Shape {
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
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("#[serde(tag = \"type\")]")
                && r.text.contains("Card(CardData)")));
    }

    #[test]
    fn services_and_operations_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &rust_casing()).is_empty());
    }

    #[test]
    fn variant_ident_pascal_cases_wire_values() {
        let config = CasingConfig::new(CaseStyle::Pascal);
        assert_eq!(variant_ident("pending", &config), "Pending");
        assert_eq!(variant_ident("card_present", &config), "CardPresent");
    }
}

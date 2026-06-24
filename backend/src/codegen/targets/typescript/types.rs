//! Building the component tree from IR shapes for the TypeScript target:
//! `emit_type` plus the IR-to-`TypeExpr` conversion and the idiomatic casing.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, type_name, wire_of};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::symbols::symbol_of;
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr, UnionDecl, Variant};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The TypeScript language key for per-language traits such as `@rename`.
pub(crate) const LANG: &str = "typescript";

/// The idiomatic TypeScript casing: camelCase fields and methods. Type names are
/// PascalCase in the IR and used as-is (not cased), and enum members and variant
/// tags are wire values kept verbatim, so only the field/method default matters.
pub fn ts_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Camel)
}

/// Convert an IR type reference into a component-tree type expression through the
/// TypeScript symbol table.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    conventions::type_expr_of(t, &symbol_of)
}

/// Emit the declaration(s) for a shape. Structures become interfaces; enums
/// become (open) literal-union types. Other shape kinds are handled by later
/// phases and emit nothing here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, LANG),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => vec![Decl::Enum(EnumDecl {
            name: type_name(shape, LANG),
            // Open-enum literals are wire tags, kept verbatim (not cased).
            members: values
                .iter()
                .map(|(value, _)| Symbol::builtin(value.clone()))
                .collect(),
        })],
        ShapeKind::Union {
            members,
            discriminator,
            ..
        } => vec![Decl::Union(UnionDecl {
            name: type_name(shape, LANG),
            discriminator: discriminator.clone(),
            variants: members.iter().map(variant_of).collect(),
        })],
        _ => vec![],
    }
}

/// Build a union variant: the tag is the member name (overridable by `@wire`),
/// and the payload is the referenced type the discriminator object intersects
/// with. The tag is a wire value, kept verbatim rather than cased.
fn variant_of(member: &Member) -> Variant {
    Variant {
        name: Symbol::builtin(member.name.clone()),
        fields: Vec::new(),
        payload: Some(type_expr_of(&member.target)),
        wire: wire_of(&member.traits),
    }
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        wire: wire_of(&member.traits),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{member, structure};
    use crate::ir::Prim;

    #[test]
    fn a_structure_becomes_an_interface_with_cased_fields() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("amount_cents", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "amountCents"
                && !i.fields[0].nullable
                && i.fields[1].name.name == "note"
                && i.fields[1].nullable));
    }

    #[test]
    fn an_enum_becomes_a_literal_union_of_verbatim_tags() {
        let shape = Shape {
            id: "billing#Status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("pending".into(), None), ("settled".into(), None)],
            },
            traits: vec![],
        };
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"));
    }

    #[test]
    fn a_union_becomes_a_discriminated_union_decl() {
        let shape = Shape {
            id: "billing#PaymentMethod".into(),
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
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Union(u)]
            if u.name.name == "PaymentMethod"
                && u.discriminator == "type"
                && u.variants.len() == 1
                && u.variants[0].name.name == "card"
                && u.variants[0].payload.is_some()));
    }

    #[test]
    fn unsupported_shape_kinds_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &ts_casing()).is_empty());
    }
}

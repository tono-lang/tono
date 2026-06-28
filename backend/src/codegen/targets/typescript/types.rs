//! Building the component tree from IR shapes for the TypeScript target:
//! `emit_type` plus the IR-to-`TypeExpr` conversion and the idiomatic casing.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, wire_of};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr, UnionDecl, Variant};
use crate::ir::{EnumBacking, Member, Shape, Tref};

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
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // An open enum is a literal-union type built from its wire values: string
        // tags for a string-backed enum, integer literals for an int-backed one.
        |backing, values, name| match backing {
            EnumBacking::String => vec![conventions::string_enum(values, name)],
            EnumBacking::Int => vec![conventions::int_enum(values, name)],
        },
        |discriminator, members, name| {
            vec![Decl::Union(UnionDecl {
                name: Symbol::builtin(name.to_string()),
                discriminator: discriminator.to_string(),
                variants: members.iter().map(variant_of).collect(),
            })]
        },
    )
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
    use crate::codegen::test_support::{
        enum_shape, int_enum_shape, member, structure, union_shape,
    };
    use crate::codegen::tree::EnumRepr;
    use crate::ir::{Prim, ShapeKind};

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
        let shape = enum_shape(
            "billing#Status",
            vec![("pending".into(), None), ("settled".into(), None)],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"
                && d.backing == EnumRepr::String));
    }

    #[test]
    fn an_int_backed_enum_becomes_a_numeric_literal_union() {
        let shape = int_enum_shape(
            "billing#http_code",
            vec![("ok".into(), Some(200)), ("error".into(), Some(500))],
        );
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "HTTPCode"
                && d.backing == EnumRepr::Int(vec![200, 500])));
    }

    #[test]
    fn a_union_becomes_a_discriminated_union_decl() {
        let shape = union_shape(
            "billing#payment_method",
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

//! Building the component tree from IR shapes for the Rust target: `emit_type`
//! plus the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become `struct` interfaces here. Enums and unions need custom
//! serde plumbing (a custom `Deserialize` for the open `Unknown` arm, a tagged
//! `enum` for unions) and are emitted as verbatim items by a later phase.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::rust::codecs::{enum_item, enum_serde_item, union_item};
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr};
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

/// Emit the type declaration(s) for a shape, which belong in the types file:
/// structures become struct interfaces; the open enum becomes its bare data-enum
/// definition; a union becomes its `#[serde(tag)]` enum (serde derives ride on the
/// type, so it is whole here). The open enum's hand-written serde impls are emitted
/// separately by [`emit_serde`]. Other shape kinds contribute nothing.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // Rust's open enum is a hand-written data enum (custom serde); the types
        // file holds only its definition, not the impls.
        |backing, values, name| vec![enum_item(backing, values, name)],
        |discriminator, members, name| vec![union_item(discriminator, members, name)],
    )
}

/// Emit the serde declaration(s) for a shape, which belong in the serde file: an
/// open enum's hand-written `as_wire`/`Serialize`/`Deserialize` impls. A structure
/// or union derives its serde on the type, so it contributes nothing here.
pub fn emit_serde(shape: &Shape) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Enum { backing, values } => {
            vec![enum_serde_item(
                backing,
                values,
                &conventions::type_ident(shape, LANG),
            )]
        }
        _ => Vec::new(),
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
    use crate::codegen::test_support::{
        enum_shape, int_enum_shape, member, structure, union_shape,
    };
    use crate::ir::{Prim, ShapeKind};

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
        let shape = enum_shape("billing#Status", vec![("pending".into(), None)]);
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Raw(r)]
            if r.text.contains("pub enum Status {") && r.text.contains("Unknown(String)")));
    }

    #[test]
    fn an_int_backed_enum_has_an_i64_definition_and_i64_serde() {
        let shape = int_enum_shape(
            "billing#http_code",
            vec![("ok".into(), Some(200)), ("error".into(), Some(500))],
        );
        // The types file holds the data enum with the i64 catch-all.
        let types = emit_type(&shape, &rust_casing());
        assert!(matches!(&types[..], [Decl::Raw(r)]
            if r.text.contains("pub enum HTTPCode {") && r.text.contains("Unknown(i64)")));
        // The serde file holds the i64 codec.
        let serde = emit_serde(&shape);
        assert!(matches!(&serde[..], [Decl::Raw(r)]
            if r.text.contains("fn as_wire(&self) -> i64")
                && r.text.contains("HTTPCode::Ok => 200,")
                && r.text.contains("s.serialize_i64(self.as_wire())")));
    }

    #[test]
    fn a_union_becomes_a_verbatim_tagged_enum_item() {
        let shape = union_shape(
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

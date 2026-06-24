//! Building the component tree from IR shapes for the Go target: `emit_type` plus
//! the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become structs whose fields carry `encoding/json` tags (the wire
//! key, plus `,string` for a 64-bit integer and `,omitempty` for an optional
//! pointer); enums become named-string types (their consts are a render concern).
//! Unions have no Go sum type, so they become an interface plus one wrapper struct
//! per variant, emitted by the codec phase. Only the union and the `@entries`
//! escape — what `encoding/json` cannot express on its own — get custom marshaling.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, wire_key};
use crate::codegen::symbol::Symbol;
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

/// Emit the declaration(s) for a shape. Structures become struct interfaces whose
/// fields carry their wire key; enums become named-string enum decls; the union's
/// interface and all codecs are emitted by the codec phase, so the union emits no
/// type here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // A Go enum is a named string built from its wire literals; the const
        // identifiers are derived at render time.
        |values, name| vec![conventions::string_enum(values, name)],
        // The union's interface is emitted alongside its codecs.
        |_discriminator, _members, _name| vec![],
    )
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config, LANG)),
        ty: conventions::entries_or_map(type_expr_of(&member.target), &member.traits),
        nullable: !member.required,
        // The wire key rides on the `encoding/json` struct tag; `render_field`
        // turns it into `json:"<wire>..."` with the right wire-encoding options.
        wire: Some(wire_key(member)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{enum_shape, member, member_with, structure, union_shape};
    use crate::ir::{Prim, ShapeKind};

    #[test]
    fn a_structure_becomes_a_struct_with_exported_fields_carrying_their_wire_key() {
        let shape = structure(
            "billing#Charge",
            vec![
                member("account_id", Tref::Prim(Prim::I64), true),
                member("note", Tref::Prim(Prim::String), false),
            ],
        );
        let decls = emit_type(&shape, &go_casing());
        // Exported PascalCase identifiers (with acronym); the field carries its wire
        // key on the tag. Nullability is preserved.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Charge"
                && i.fields[0].name.name == "AccountID"
                && i.fields[0].wire.as_deref() == Some("account_id")
                && !i.fields[0].nullable
                && i.fields[1].name.name == "Note"
                && i.fields[1].wire.as_deref() == Some("note")
                && i.fields[1].nullable));
    }

    #[test]
    fn a_wire_override_rides_on_the_field_tag() {
        let shape = structure(
            "billing#Charge",
            vec![member_with(
                "amount_cents",
                Tref::Prim(Prim::I64),
                true,
                vec![crate::ir::Trait {
                    id: "core#wire".into(),
                    value: serde_json::json!("amount"),
                }],
            )],
        );
        let decls = emit_type(&shape, &go_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "AmountCents"
                && i.fields[0].wire.as_deref() == Some("amount")));
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
    fn a_union_emits_no_type_here_the_codec_phase_emits_its_sealed_interface() {
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
        assert!(emit_type(&union, &go_casing()).is_empty());
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

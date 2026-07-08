//! Building the component tree from IR shapes for the Go target: `emit_type` plus
//! the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become structs whose fields carry `encoding/json` tags (the wire
//! key, plus `,string` for a 64-bit integer and `,omitempty` for an optional
//! pointer); enums become named-string types (their consts are a render concern).
//! Unions have no Go sum type, so they become an interface plus one wrapper struct
//! per variant (each with its marker method) — all type declarations, emitted here.
//! Their serialization (the wrapper `MarshalJSON`s and the `unmarshalX` dispatcher),
//! like the `@entries` escape, lives in the serde phase: only the union and
//! `@entries` need custom marshaling, what `encoding/json` cannot express alone.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::conventions::{self, field_ident, wire_key};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::union_type_decls;
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, TypeExpr};
use crate::ir::{Member, Shape, Tref};

/// The Go language key for per-language traits such as `@rename`.
pub(crate) const LANG: &str = "go";

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

/// Emit the type declaration(s) for a shape. Structures become struct interfaces
/// whose fields carry their wire key; enums become named-string enum decls; a union
/// becomes its marker interface plus one wrapper struct per variant (each with its
/// `is<Union>()` marker). The union's serialization — the wrapper `MarshalJSON`s and
/// the `unmarshalX` dispatcher — is emitted by the serde phase, not here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    conventions::emit_shape(
        shape,
        LANG,
        |m| field_of(m, config),
        // A Go enum is a named string or int built from its wire values; the const
        // identifiers are derived at render time.
        |backing, values, name| vec![conventions::open_enum(backing, values, name)],
        // The interface, wrappers, and markers are types; their serde lives in the
        // serde phase.
        |_discriminator, members, _name| union_type_decls(shape, members),
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
    use crate::codegen::test_support::{
        enum_shape, int_enum_shape, member, member_with, structure, union_shape,
    };
    use crate::codegen::tree::EnumRepr;
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
                && d.members[1].name == "settled"
                && d.backing == EnumRepr::String));
    }

    #[test]
    fn an_int_backed_enum_becomes_a_named_int_carrying_its_integers() {
        let shape = int_enum_shape(
            "billing#http_code",
            vec![("ok".into(), Some(200)), ("error".into(), Some(500))],
        );
        let decls = emit_type(&shape, &go_casing());
        assert!(matches!(&decls[..], [Decl::Enum(d)]
            if d.name.name == "HTTPCode"
                && d.members[0].name == "ok"
                && d.members[1].name == "error"
                && d.backing == EnumRepr::Int(vec![200, 500])));
    }

    #[test]
    fn a_union_emits_its_interface_wrappers_and_markers_but_no_serde() {
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
        let out: String = decls
            .iter()
            .map(|d| {
                crate::codegen::target::RenderRules::render_decl(
                    &crate::codegen::targets::go::GoRules,
                    d,
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        // The marker interface, the wrapper struct, and the marker method are types;
        // the serialization (MarshalJSON / unmarshalX) belongs to the serde phase.
        assert!(out.contains("type Method interface{ isMethod() }"));
        assert!(out.contains("type MethodCard struct{ Value CardData }"));
        assert!(out.contains("func (MethodCard) isMethod() {}"));
        assert!(!out.contains("MarshalJSON"));
        assert!(!out.contains("unmarshalMethod"));
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

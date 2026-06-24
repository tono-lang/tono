//! Building the component tree from IR shapes for the Go target: `emit_type` plus
//! the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become structs and enums become named-string types (their consts
//! are a render concern). Unions have no Go sum type, so they are emitted as a
//! struct with one pointer per variant plus custom JSON methods by a later phase.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The Go language key for per-language traits such as `@rename`.
const GO_LANG: &str = "go";

/// The idiomatic Go casing: exported PascalCase fields (so `encoding/json` can
/// see them). Type names are PascalCase in the IR and used as-is; enum values are
/// wire strings kept verbatim, so only the field default matters here.
pub fn go_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Pascal)
}

/// Convert an IR type reference into a component-tree type expression, resolving
/// leaf types through the Go symbol table. Collections and generic applications
/// become structural `TypeExpr` nodes.
pub fn type_expr_of(t: &Tref) -> TypeExpr {
    match t {
        Tref::List(inner) => TypeExpr::list(type_expr_of(inner)),
        Tref::Map(key, value) => TypeExpr::map(type_expr_of(key), type_expr_of(value)),
        Tref::Ref { args, .. } if !args.is_empty() => {
            TypeExpr::Generic(symbol_of(t), args.iter().map(type_expr_of).collect())
        }
        Tref::Prim(_) | Tref::Param(_) | Tref::Ref { .. } => TypeExpr::Ref(symbol_of(t)),
    }
}

/// Emit the declaration(s) for a shape. Structures become struct interfaces;
/// enums become named-string enum decls; unions are emitted by a later phase.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, config),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => vec![Decl::Enum(EnumDecl {
            name: type_name(shape, config),
            // Enum values are wire strings kept verbatim; the const identifiers
            // are derived at render time.
            members: values
                .iter()
                .map(|(value, _)| Symbol::builtin(value.clone()))
                .collect(),
        })],
        _ => vec![],
    }
}

/// The identifier for a shape's own name (after the `module#` prefix). Type names
/// are PascalCase in the IR, so they are used as-is; only a Go `@rename`
/// overrides the identifier.
pub(crate) fn type_ident(shape: &Shape, _config: &CasingConfig) -> String {
    let local = shape.id.rsplit('#').next().unwrap_or(&shape.id);
    rename_of(&shape.traits).unwrap_or_else(|| local.to_string())
}

/// The exported PascalCase identifier for a member, honoring a Go `@rename`. This
/// is the in-code name, independent of the wire key carried in the json tag.
pub(crate) fn field_ident(member: &Member, config: &CasingConfig) -> String {
    casing::transform(
        &member.name,
        SymbolKind::Field,
        config,
        rename_of(&member.traits).as_deref(),
    )
}

/// The serialization key for a member: its `@wire` override, else the canonical
/// name. Always carried on a Go field, since the exported identifier differs from
/// the wire key.
pub(crate) fn wire_key(member: &Member) -> String {
    wire_of(&member.traits).unwrap_or_else(|| member.name.clone())
}

fn type_name(shape: &Shape, config: &CasingConfig) -> Symbol {
    Symbol::builtin(type_ident(shape, config))
}

fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(field_ident(member, config)),
        ty: type_expr_of(&member.target),
        nullable: !member.required,
        // Go always carries the wire key in the json tag.
        wire: Some(wire_key(member)),
    }
}

/// Read the `@rename` identifier override for Go (trait id `core#rename`, a value
/// object keyed by language) from a trait set.
fn rename_of(traits: &[crate::ir::Trait]) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#rename")
        .and_then(|t| t.value.get(GO_LANG))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Read the `@wire` override (trait id `core#wire`) from a member's traits.
fn wire_of(traits: &[crate::ir::Trait]) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#wire")
        .and_then(|t| t.value.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Prim, Trait};
    use serde_json::json;

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

    #[test]
    fn type_expr_resolves_collections_and_generics() {
        assert_eq!(
            type_expr_of(&Tref::List(Box::new(Tref::Prim(Prim::String)))),
            TypeExpr::list(TypeExpr::Ref(Symbol::builtin("string")))
        );
        assert_eq!(
            type_expr_of(&Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::I64)),
            )),
            TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("string")),
                TypeExpr::Ref(Symbol::builtin("int64")),
            )
        );
        let generic = type_expr_of(&Tref::Ref {
            id: "core#Page".into(),
            args: vec![Tref::Ref {
                id: "p#Charge".into(),
                args: vec![],
            }],
        });
        assert!(
            matches!(&generic, TypeExpr::Generic(head, args) if head.name == "Page" && args.len() == 1),
            "expected Page<_>, got {generic:?}"
        );
    }

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
    fn a_wire_override_sets_the_tag_key_independently_of_the_identifier() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#wire".into(),
            value: json!("amount"),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &go_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "AmountCents"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn rename_overrides_the_identifier_for_go_only() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#rename".into(),
            value: json!({ "go": "AmountCentsV2", "rust": "amount_cents" }),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &go_casing());
        // The Go rename wins; the wire key stays the canonical snake name.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "AmountCentsV2"
                && i.fields[0].wire.as_deref() == Some("amount_cents")));
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
    fn unions_and_services_emit_nothing_yet() {
        let union = Shape {
            id: "billing#Method".into(),
            kind: ShapeKind::Union {
                params: vec![],
                discriminator: "type".into(),
                members: vec![],
            },
            traits: vec![],
        };
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&union, &go_casing()).is_empty());
        assert!(emit_type(&service, &go_casing()).is_empty());
    }
}

//! Building the component tree from IR shapes for the TypeScript target:
//! `emit_type` plus the IR-to-`TypeExpr` conversion and the idiomatic casing.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::typescript::symbols::symbol_of;
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The idiomatic TypeScript casing: PascalCase types, camelCase fields and
/// methods. Enum members are not cased here: an open-enum literal is the wire tag
/// itself, kept verbatim.
pub fn ts_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Camel)
        .with(SymbolKind::Type, CaseStyle::Pascal)
        .with(SymbolKind::Variant, CaseStyle::Pascal)
}

/// Convert an IR type reference into a component-tree type expression, resolving
/// leaf types through the TypeScript symbol table. Collections and generic
/// applications become structural `TypeExpr` nodes.
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

/// Emit the declaration(s) for a shape. Structures become interfaces; enums
/// become (open) literal-union types. Other shape kinds are handled by later
/// phases and emit nothing here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(&shape.id, config),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => vec![Decl::Enum(EnumDecl {
            name: type_name(&shape.id, config),
            // Open-enum literals are wire tags, kept verbatim (not cased).
            members: values
                .iter()
                .map(|(value, _)| Symbol::builtin(value.clone()))
                .collect(),
        })],
        _ => vec![],
    }
}

/// The PascalCase symbol for a shape's own name (defined locally, so not
/// imported), derived from the canonical name after the `module#` prefix.
fn type_name(id: &str, config: &CasingConfig) -> Symbol {
    let local = id.rsplit('#').next().unwrap_or(id);
    Symbol::builtin(casing::transform(local, SymbolKind::Type, config, None))
}

/// Build a field node: a camelCased identifier, the field's type expression,
/// nullability from `required`, and any `@wire` serialization-key override.
fn field_of(member: &Member, config: &CasingConfig) -> Field {
    Field {
        name: Symbol::builtin(casing::transform(
            &member.name,
            SymbolKind::Field,
            config,
            None,
        )),
        ty: type_expr_of(&member.target),
        nullable: !member.required,
        wire: wire_of(&member.traits),
    }
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
                TypeExpr::Ref(Symbol::builtin("bigint")),
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
    fn a_structure_becomes_an_interface_with_cased_fields() {
        let shape = structure(
            "billing#charge",
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
    fn a_member_wire_override_is_carried_on_the_field() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#wire".into(),
            value: json!("amount"),
        }];
        let shape = structure("billing#charge", vec![m]);
        let decls = emit_type(&shape, &ts_casing());
        // The identifier is cased; the wire key is the override, independently.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "amountCents"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn an_enum_becomes_a_literal_union_of_verbatim_tags() {
        let shape = Shape {
            id: "billing#status".into(),
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
    fn unsupported_shape_kinds_emit_nothing() {
        let service = Shape {
            id: "billing#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_type(&service, &ts_casing()).is_empty());
    }
}

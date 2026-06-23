//! Building the component tree from IR shapes for the TypeScript target:
//! `emit_type` plus the IR-to-`TypeExpr` conversion and the idiomatic casing.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::typescript::symbols::symbol_of;
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr, UnionDecl, Variant};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The idiomatic TypeScript casing: camelCase fields and methods. Type names are
/// PascalCase in the IR and used as-is (not cased), and enum members and variant
/// tags are wire values kept verbatim, so only the field/method default matters.
pub fn ts_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Camel)
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
            name: type_name(shape, config),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        ShapeKind::Enum { values, .. } => vec![Decl::Enum(EnumDecl {
            name: type_name(shape, config),
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
            name: type_name(shape, config),
            discriminator: discriminator.clone(),
            variants: members.iter().map(variant_of).collect(),
        })],
        _ => vec![],
    }
}

/// The TypeScript language key for per-language traits such as `@rename`.
const TS_LANG: &str = "typescript";

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

/// The identifier for a shape's own name (after the `module#` prefix). Type
/// names are PascalCase in the IR, so they are used as-is (casing them would
/// corrupt multi-word names like `KitchenSink`); only a TypeScript `@rename`
/// overrides the identifier. The casing config is unused for types but kept for
/// signature symmetry with the field path.
pub(crate) fn type_ident(shape: &Shape, _config: &CasingConfig) -> String {
    let local = shape.id.rsplit('#').next().unwrap_or(&shape.id);
    rename_of(&shape.traits).unwrap_or_else(|| local.to_string())
}

/// The camelCase identifier for a member, honoring a TypeScript `@rename`. This
/// is the in-code name, independent of the wire key.
pub(crate) fn field_ident(member: &Member, config: &CasingConfig) -> String {
    casing::transform(
        &member.name,
        SymbolKind::Field,
        config,
        rename_of(&member.traits).as_deref(),
    )
}

/// The serialization key for a member: its `@wire` override, else the canonical
/// name. Independent of the in-code identifier.
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
        wire: wire_of(&member.traits),
    }
}

/// Read the `@rename` identifier override for TypeScript (trait id `core#rename`,
/// a value object keyed by language) from a trait set.
fn rename_of(traits: &[crate::ir::Trait]) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#rename")
        .and_then(|t| t.value.get(TS_LANG))
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
    fn a_member_wire_override_is_carried_on_the_field() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#wire".into(),
            value: json!("amount"),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &ts_casing());
        // The identifier is cased; the wire key is the override, independently.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "amountCents"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn rename_overrides_the_identifier_independently_of_wire() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![
            Trait {
                id: "core#rename".into(),
                value: json!({ "typescript": "amountCentsV2" }),
            },
            Trait {
                id: "core#wire".into(),
                value: json!("amount"),
            },
        ];
        let shape = Shape {
            id: "billing#Charge".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![m],
            },
            traits: vec![Trait {
                id: "core#rename".into(),
                value: json!({ "typescript": "Invoice" }),
            }],
        };
        let decls = emit_type(&shape, &ts_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Invoice"
                && i.fields[0].name.name == "amountCentsV2"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn rename_for_another_language_is_ignored() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#rename".into(),
            value: json!({ "rust": "amount_cents" }),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &ts_casing());
        // No TypeScript rename, so the camelCase default applies.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "amountCents"));
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

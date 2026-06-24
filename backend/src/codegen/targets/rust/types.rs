//! Building the component tree from IR shapes for the Rust target: `emit_type`
//! plus the IR-to-`TypeExpr` conversion and the idiomatic casing.
//!
//! Structures become `struct` interfaces here. Enums and unions need custom
//! serde plumbing (a custom `Deserialize` for the open `Unknown` arm, a tagged
//! `enum` for unions) and are emitted as verbatim items by a later phase.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::tree::{Decl, Field, Interface, TypeExpr};
use crate::ir::{Member, Shape, ShapeKind, Tref};

/// The Rust language key for per-language traits such as `@rename`.
const RUST_LANG: &str = "rust";

/// The idiomatic Rust casing: snake_case fields. Type names are PascalCase in the
/// IR and used as-is (not cased); only the field default matters here.
pub fn rust_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Snake)
}

/// Convert an IR type reference into a component-tree type expression, resolving
/// leaf types through the Rust symbol table. Collections and generic applications
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
/// enums and unions are emitted by a later phase (they need custom serde
/// plumbing) and contribute nothing here.
pub fn emit_type(shape: &Shape, config: &CasingConfig) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, config),
            fields: members.iter().map(|m| field_of(m, config)).collect(),
        })],
        _ => vec![],
    }
}

/// The identifier for a shape's own name (after the `module#` prefix). Type names
/// are PascalCase in the IR, so they are used as-is (casing them would corrupt
/// multi-word names like `KitchenSink`); only a Rust `@rename` overrides the
/// identifier. The casing config is unused for types but kept for signature
/// symmetry with the field path.
pub(crate) fn type_ident(shape: &Shape, _config: &CasingConfig) -> String {
    let local = shape.id.rsplit('#').next().unwrap_or(&shape.id);
    rename_of(&shape.traits).unwrap_or_else(|| local.to_string())
}

/// The snake_case identifier for a member, honoring a Rust `@rename`. This is the
/// in-code name, independent of the wire key.
pub(crate) fn field_ident(member: &Member, config: &CasingConfig) -> String {
    casing::transform(
        &member.name,
        SymbolKind::Field,
        config,
        rename_of(&member.traits).as_deref(),
    )
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

/// Read the `@rename` identifier override for Rust (trait id `core#rename`, a
/// value object keyed by language) from a trait set.
fn rename_of(traits: &[crate::ir::Trait]) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#rename")
        .and_then(|t| t.value.get(RUST_LANG))
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
            TypeExpr::list(TypeExpr::Ref(Symbol::builtin("String")))
        );
        assert_eq!(
            type_expr_of(&Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::I64)),
            )),
            TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("String")),
                TypeExpr::Ref(Symbol::builtin("i64")),
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
    fn a_member_wire_override_is_carried_on_the_field() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#wire".into(),
            value: json!("amount"),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &rust_casing());
        // The identifier is cased; the wire key is the override, independently.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "amount_cents"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn rename_overrides_the_identifier_independently_of_wire() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![
            Trait {
                id: "core#rename".into(),
                value: json!({ "rust": "amount_cents_v2" }),
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
                value: json!({ "rust": "Invoice" }),
            }],
        };
        let decls = emit_type(&shape, &rust_casing());
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Invoice"
                && i.fields[0].name.name == "amount_cents_v2"
                && i.fields[0].wire.as_deref() == Some("amount")));
    }

    #[test]
    fn rename_for_another_language_is_ignored() {
        let mut m = member("amount_cents", Tref::Prim(Prim::I64), true);
        m.traits = vec![Trait {
            id: "core#rename".into(),
            value: json!({ "typescript": "amountCents" }),
        }];
        let shape = structure("billing#Charge", vec![m]);
        let decls = emit_type(&shape, &rust_casing());
        // No Rust rename, so the snake_case default applies.
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.fields[0].name.name == "amount_cents"));
    }

    #[test]
    fn enums_and_unions_emit_nothing_yet() {
        let enum_shape = Shape {
            id: "billing#Status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("pending".into(), None)],
            },
            traits: vec![],
        };
        let union_shape = Shape {
            id: "billing#Method".into(),
            kind: ShapeKind::Union {
                params: vec![],
                discriminator: "type".into(),
                members: vec![],
            },
            traits: vec![],
        };
        assert!(emit_type(&enum_shape, &rust_casing()).is_empty());
        assert!(emit_type(&union_shape, &rust_casing()).is_empty());
    }
}

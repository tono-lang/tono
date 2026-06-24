//! Language-agnostic conventions every target reuses.
//!
//! Adding a target should mean declaring only what is genuinely
//! language-specific — its primitive mapping, casing defaults, render rules, and
//! codecs. The cross-cutting boilerplate lives here: reading the naming and wire
//! traits off IR members and shapes, the nominal-reference symbol, and the
//! IR-to-`TypeExpr` skeleton (parameterized by the target's `symbol_of`). Keeping
//! it in one place is what stops every new language from re-deriving the same
//! trait plumbing.

use crate::codegen::casing::{self, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::tree::TypeExpr;
use crate::ir::{Member, Shape, Trait, Tref};

/// The `@rename(lang)` identifier override (trait `core#rename`, a value object
/// keyed by language). Replaces the in-code identifier only; never the wire key.
pub fn rename_of(traits: &[Trait], lang: &str) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#rename")
        .and_then(|t| t.value.get(lang))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// The `@wire` serialization-key override (trait `core#wire`). Replaces the wire
/// key only; never the in-code identifier.
pub fn wire_of(traits: &[Trait]) -> Option<String> {
    traits
        .iter()
        .find(|t| t.id == "core#wire")
        .and_then(|t| t.value.as_str())
        .map(str::to_string)
}

/// The serialization key for a member: its `@wire` override, else the canonical
/// name. Independent of the in-code identifier.
pub fn wire_key(member: &Member) -> String {
    wire_of(&member.traits).unwrap_or_else(|| member.name.clone())
}

/// Whether a member carries the `@entries` map-escape trait (`core#entries`).
pub fn has_entries(traits: &[Trait]) -> bool {
    traits.iter().any(|t| t.id == "core#entries")
}

/// Reshape a map into an `@entries` pairs-array when the member carries the
/// `core#entries` trait; any other type is unchanged. The escape only applies to
/// a map (a non-map `@entries` is rejected upstream by the typechecker).
pub fn entries_or_map(ty: TypeExpr, traits: &[Trait]) -> TypeExpr {
    match ty {
        TypeExpr::Map(key, value) if has_entries(traits) => TypeExpr::Entries(key, value),
        other => other,
    }
}

/// The identifier for a shape's own name (after the `module#` prefix). Type names
/// are PascalCase in the IR, so they are used as-is (casing them would corrupt
/// multi-word names like `KitchenSink`); only a `@rename(lang)` overrides it.
pub fn type_ident(shape: &Shape, lang: &str) -> String {
    let local = shape.id.rsplit('#').next().unwrap_or(&shape.id);
    rename_of(&shape.traits, lang).unwrap_or_else(|| local.to_string())
}

/// A symbol for a shape's own name.
pub fn type_name(shape: &Shape, lang: &str) -> Symbol {
    Symbol::builtin(type_ident(shape, lang))
}

/// The cased identifier for a member, honoring a `@rename(lang)`. This is the
/// in-code name, independent of the wire key; the casing style comes from the
/// target's config.
pub fn field_ident(member: &Member, config: &CasingConfig, lang: &str) -> String {
    casing::transform(
        &member.name,
        SymbolKind::Field,
        config,
        rename_of(&member.traits, lang).as_deref(),
    )
}

/// A nominal reference `module#Name` becomes a symbol imported from `module`; an
/// id without a module separator is treated as an in-scope name.
pub fn ref_symbol(id: &str) -> Symbol {
    match id.split_once('#') {
        Some((module, name)) => Symbol::imported(name, module, name),
        None => Symbol::builtin(id),
    }
}

/// Convert an IR type reference into a component-tree type expression, resolving
/// leaf types through the target's `symbol_of`. Collections and generic
/// applications become structural `TypeExpr` nodes; this skeleton is identical
/// across targets, so only `symbol_of` varies.
pub fn type_expr_of(t: &Tref, symbol_of: &impl Fn(&Tref) -> Symbol) -> TypeExpr {
    match t {
        Tref::List(inner) => TypeExpr::list(type_expr_of(inner, symbol_of)),
        Tref::Map(key, value) => {
            TypeExpr::map(type_expr_of(key, symbol_of), type_expr_of(value, symbol_of))
        }
        Tref::Ref { args, .. } if !args.is_empty() => TypeExpr::Generic(
            symbol_of(t),
            args.iter().map(|a| type_expr_of(a, symbol_of)).collect(),
        ),
        Tref::Prim(_) | Tref::Param(_) | Tref::Ref { .. } => TypeExpr::Ref(symbol_of(t)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::casing::CaseStyle;
    use crate::codegen::symbol::Import;
    use crate::ir::ShapeKind;
    use serde_json::json;

    fn member(name: &str, traits: Vec<Trait>) -> Member {
        Member {
            name: name.into(),
            target: Tref::Prim(crate::ir::Prim::String),
            required: true,
            default: None,
            constraints: vec![],
            traits,
        }
    }

    fn trait_of(id: &str, value: serde_json::Value) -> Trait {
        Trait {
            id: id.into(),
            value,
        }
    }

    #[test]
    fn rename_is_language_scoped() {
        let traits = vec![trait_of(
            "core#rename",
            json!({ "rust": "renamed_rs", "go": "RenamedGo" }),
        )];
        assert_eq!(rename_of(&traits, "rust").as_deref(), Some("renamed_rs"));
        assert_eq!(rename_of(&traits, "go").as_deref(), Some("RenamedGo"));
        assert_eq!(rename_of(&traits, "typescript"), None);
        assert_eq!(rename_of(&[], "rust"), None);
    }

    #[test]
    fn wire_key_falls_back_to_the_canonical_name() {
        assert_eq!(wire_key(&member("amount_cents", vec![])), "amount_cents");
        assert_eq!(
            wire_key(&member(
                "amount_cents",
                vec![trait_of("core#wire", json!("amount"))]
            )),
            "amount"
        );
    }

    #[test]
    fn entries_reshapes_only_a_map_with_the_trait() {
        let map = || {
            TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("k")),
                TypeExpr::Ref(Symbol::builtin("v")),
            )
        };
        let entries = vec![trait_of("core#entries", json!(true))];
        assert!(has_entries(&entries));
        assert!(matches!(
            entries_or_map(map(), &entries),
            TypeExpr::Entries(_, _)
        ));
        // Without the trait, or for a non-map, the type is unchanged.
        assert!(matches!(entries_or_map(map(), &[]), TypeExpr::Map(_, _)));
        let scalar = TypeExpr::Ref(Symbol::builtin("x"));
        assert!(matches!(entries_or_map(scalar, &entries), TypeExpr::Ref(_)));
    }

    #[test]
    fn type_ident_uses_the_local_name_unless_renamed() {
        let shape = |traits: Vec<Trait>| Shape {
            id: "billing#KitchenSink".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits,
        };
        assert_eq!(type_ident(&shape(vec![]), "rust"), "KitchenSink");
        assert_eq!(
            type_ident(
                &shape(vec![trait_of("core#rename", json!({ "rust": "Invoice" }))]),
                "rust"
            ),
            "Invoice"
        );
        assert_eq!(type_name(&shape(vec![]), "go").name, "KitchenSink");
    }

    #[test]
    fn field_ident_cases_and_honors_rename() {
        let snake = CasingConfig::new(CaseStyle::Snake);
        let pascal = CasingConfig::new(CaseStyle::Pascal);
        assert_eq!(
            field_ident(&member("amount_cents", vec![]), &snake, "rust"),
            "amount_cents"
        );
        assert_eq!(
            field_ident(&member("amount_cents", vec![]), &pascal, "go"),
            "AmountCents"
        );
        assert_eq!(
            field_ident(
                &member(
                    "amount_cents",
                    vec![trait_of("core#rename", json!({ "rust": "amountCentsV2" }))]
                ),
                &snake,
                "rust"
            ),
            "amountCentsV2"
        );
    }

    #[test]
    fn ref_symbol_imports_nominal_refs_and_keeps_bare_names_local() {
        let imported = ref_symbol("payments#Charge");
        assert_eq!(imported.name, "Charge");
        assert_eq!(
            imported.import,
            Some(Import {
                module: "payments".into(),
                imported: "Charge".into(),
            })
        );
        let bare = ref_symbol("Bare");
        assert_eq!(bare.name, "Bare");
        assert_eq!(bare.import, None);
    }

    #[test]
    fn type_expr_of_resolves_collections_and_generics_through_symbol_of() {
        // A trivial symbol_of that names a ref by its local part.
        let symbol_of = |t: &Tref| match t {
            Tref::Ref { id, .. } => ref_symbol(id),
            Tref::List(_) => Symbol::builtin("List"),
            Tref::Map(_, _) => Symbol::builtin("Map"),
            Tref::Prim(_) => Symbol::builtin("prim"),
            Tref::Param(n) => Symbol::builtin(n.clone()),
        };
        assert_eq!(
            type_expr_of(
                &Tref::List(Box::new(Tref::Prim(crate::ir::Prim::Bool))),
                &symbol_of
            ),
            TypeExpr::list(TypeExpr::Ref(Symbol::builtin("prim")))
        );
        let generic = type_expr_of(
            &Tref::Ref {
                id: "core#Page".into(),
                args: vec![Tref::Ref {
                    id: "p#Item".into(),
                    args: vec![],
                }],
            },
            &symbol_of,
        );
        assert!(matches!(&generic, TypeExpr::Generic(head, args)
            if head.name == "Page" && args.len() == 1));
    }
}

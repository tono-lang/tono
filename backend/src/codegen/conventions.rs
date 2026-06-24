//! Language-agnostic conventions every target reuses.
//!
//! Adding a target should mean declaring only what is genuinely
//! language-specific — its primitive mapping, casing defaults, render rules, and
//! codecs. The cross-cutting boilerplate lives here: reading the naming and wire
//! traits off IR members and shapes, the nominal-reference symbol, and the
//! IR-to-`TypeExpr` skeleton (parameterized by the target's `symbol_of`). Keeping
//! it in one place is what stops every new language from re-deriving the same
//! trait plumbing.

use crate::codegen::casing::{self, CaseStyle, CasingConfig};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::tree::{Decl, EnumDecl, Field, Interface, TypeExpr};
use crate::ir::{Member, Prim, Shape, ShapeKind, Trait, Tref};

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

/// Case a snake_case type name to PascalCase — the spelling every current target
/// uses for type identifiers — honoring the default initialism set. The IR carries
/// type names in snake_case (the frontend requires it), exactly like field and
/// member names, so they ride the same casing engine.
fn type_case(name: &str) -> String {
    casing::transform(
        name,
        SymbolKind::Type,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    )
}

/// The identifier for a shape's own name (after the `module#` prefix), cased to
/// PascalCase; a `@rename(lang)` overrides it verbatim.
pub fn type_ident(shape: &Shape, lang: &str) -> String {
    let local = shape.id.rsplit('#').next().unwrap_or(&shape.id);
    rename_of(&shape.traits, lang).unwrap_or_else(|| type_case(local))
}

/// A symbol for a shape's own name.
pub fn type_name(shape: &Shape, lang: &str) -> Symbol {
    Symbol::builtin(type_ident(shape, lang))
}

/// The PascalCase in-code identifier for a type id (`module#name`, or a bare
/// name), matching how the type is defined and referenced. Used where only the id
/// is in hand (e.g. naming a payload's codec).
pub fn type_ident_from_id(id: &str) -> String {
    type_case(id.rsplit('#').next().unwrap_or(id))
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

/// A nominal reference `module#name` becomes a symbol imported from `module`; an
/// id without a module separator is treated as an in-scope name. The name is cased
/// to PascalCase so a reference matches the type's own (also cased) definition.
pub fn ref_symbol(id: &str) -> Symbol {
    match id.split_once('#') {
        Some((module, name)) => {
            let cased = type_case(name);
            Symbol::imported(cased.clone(), module, cased)
        }
        None => Symbol::builtin(type_case(id)),
    }
}

/// Map an IR type reference to a target's leaf symbol. The dispatch is identical
/// across targets — a primitive goes through the target's own `prim_symbol`
/// table, a param is a bare identifier, a nominal ref resolves through
/// [`ref_symbol`] — so only the per-language pieces vary: the primitive table and
/// the structural-collection names. Collections have no single nominal symbol
/// (they are built as `TypeExpr` nodes); the `list`/`map` names are fallbacks that
/// keep the function total when a collection ref is passed directly.
pub fn leaf_symbol_of(
    t: &Tref,
    prim_symbol: impl Fn(&Prim) -> Symbol,
    list: &str,
    map: &str,
) -> Symbol {
    match t {
        Tref::Prim(p) => prim_symbol(p),
        Tref::Param(name) => Symbol::builtin(name.clone()),
        Tref::Ref { id, .. } => ref_symbol(id),
        Tref::List(_) => Symbol::builtin(list),
        Tref::Map(_, _) => Symbol::builtin(map),
    }
}

/// The in-code spelling of a primitive in each target. Kept as one table because
/// the mapping is data, not structure: the well-known types are identical across
/// languages, and the integer/float/bytes spellings differ only by token, so a
/// single source of truth is clearer than three parallel match arms — and a target
/// selects only its own field. Wire encoding (64-bit ints as strings, bytes as
/// base64) is a codec concern handled elsewhere; this is purely the type name.
pub struct PrimSpelling {
    pub rust: &'static str,
    pub go: &'static str,
    pub typescript: &'static str,
}

/// The per-language spelling of a primitive. Integers map to their exact-width
/// type in Rust and Go (64-bit included, held natively); TypeScript has only
/// `number` (precise to 2^53) and `bigint`, so the wide integers become `bigint`
/// and the rest `number`. `bytes` is the language's byte buffer, and the
/// well-known types are branded wrappers named for their kind.
pub fn prim_spelling(p: &Prim) -> PrimSpelling {
    let (rust, go, typescript) = match p {
        Prim::Bool => ("bool", "bool", "boolean"),
        Prim::String => ("String", "string", "string"),
        Prim::Bytes => ("Vec<u8>", "[]byte", "Uint8Array"),
        Prim::I8 => ("i8", "int8", "number"),
        Prim::I16 => ("i16", "int16", "number"),
        Prim::I32 => ("i32", "int32", "number"),
        Prim::I64 => ("i64", "int64", "bigint"),
        Prim::U8 => ("u8", "uint8", "number"),
        Prim::U16 => ("u16", "uint16", "number"),
        Prim::U32 => ("u32", "uint32", "number"),
        Prim::U64 => ("u64", "uint64", "bigint"),
        Prim::Float => ("f64", "float64", "number"),
        Prim::Timestamp => ("Timestamp", "Timestamp", "Timestamp"),
        Prim::Date => ("LocalDate", "LocalDate", "LocalDate"),
        Prim::Duration => ("Duration", "Duration", "Duration"),
        Prim::Uuid => ("Uuid", "Uuid", "Uuid"),
    };
    PrimSpelling {
        rust,
        go,
        typescript,
    }
}

/// Emit the declaration(s) for a shape. The dispatch over shape kinds is the same
/// for every target — a structure is always an interface of fields, an enum and a
/// union are always built from the shape's name, and other kinds emit nothing — so
/// only the per-language policies vary: how a field carries its wire key
/// (`field_of`), and how an enum and a union are spelled (`emit_enum`,
/// `emit_union`). The name passed to the enum/union policies is the shape's
/// own identifier (after any `@rename`).
pub fn emit_shape(
    shape: &Shape,
    lang: &str,
    field_of: impl Fn(&Member) -> Field,
    emit_enum: impl Fn(&[(String, Option<i64>)], &str) -> Vec<Decl>,
    emit_union: impl Fn(&str, &[Member], &str) -> Vec<Decl>,
) -> Vec<Decl> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } => vec![Decl::Interface(Interface {
            name: type_name(shape, lang),
            fields: members.iter().map(&field_of).collect(),
        })],
        ShapeKind::Enum { values, .. } => emit_enum(values, &type_ident(shape, lang)),
        ShapeKind::Union {
            discriminator,
            members,
            ..
        } => emit_union(discriminator, members, &type_ident(shape, lang)),
        _ => vec![],
    }
}

/// An open enum as a named list of its wire literals: the representation Go (a
/// named string) and TypeScript (a literal union) share. Rust instead needs a
/// hand-written `Deserialize` for its `Unknown` arm, so it does not use this. The
/// literals are wire tags kept verbatim; their in-code form is a render concern.
pub fn string_enum(values: &[(String, Option<i64>)], name: &str) -> Decl {
    Decl::Enum(EnumDecl {
        name: Symbol::builtin(name.to_string()),
        members: values
            .iter()
            .map(|(value, _)| Symbol::builtin(value.clone()))
            .collect(),
    })
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
            id: "billing#kitchen_sink".into(),
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
        // The snake_case id is cased to PascalCase, so a reference matches the
        // type's own definition; the imported name is cased too.
        let imported = ref_symbol("payments#card_data");
        assert_eq!(imported.name, "CardData");
        assert_eq!(
            imported.import,
            Some(Import {
                module: "payments".into(),
                imported: "CardData".into(),
            })
        );
        let bare = ref_symbol("bare_thing");
        assert_eq!(bare.name, "BareThing");
        assert_eq!(bare.import, None);
    }

    #[test]
    fn type_ident_casing_handles_multiword_and_acronyms() {
        // The shared casing the definition and every reference go through: a
        // snake_case id to PascalCase, with the initialism set re-upcasing `http`.
        assert_eq!(
            type_ident_from_id("billing#payment_method"),
            "PaymentMethod"
        );
        assert_eq!(type_ident_from_id("billing#http_code"), "HTTPCode");
        assert_eq!(type_ident_from_id("charge"), "Charge");
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

    #[test]
    fn leaf_symbol_of_dispatches_each_reference_kind() {
        let prim = |p: &Prim| Symbol::builtin(format!("{p:?}"));
        // A primitive goes through the supplied table; a param is a bare name; a
        // nominal ref is imported; collections fall back to the structural names.
        assert_eq!(
            leaf_symbol_of(&Tref::Prim(Prim::Bool), prim, "List", "Map").name,
            "Bool"
        );
        let param = leaf_symbol_of(&Tref::Param("T".into()), prim, "List", "Map");
        assert_eq!(param.name, "T");
        assert_eq!(param.import, None);
        let reference = leaf_symbol_of(
            &Tref::Ref {
                id: "pay#Charge".into(),
                args: vec![],
            },
            prim,
            "List",
            "Map",
        );
        assert_eq!(reference.name, "Charge");
        assert!(reference.import.is_some());
        assert_eq!(
            leaf_symbol_of(
                &Tref::List(Box::new(Tref::Prim(Prim::Bool))),
                prim,
                "List",
                "Map"
            )
            .name,
            "List"
        );
        assert_eq!(
            leaf_symbol_of(
                &Tref::Map(
                    Box::new(Tref::Prim(Prim::String)),
                    Box::new(Tref::Prim(Prim::Bool)),
                ),
                prim,
                "List",
                "Map",
            )
            .name,
            "Map"
        );
    }

    #[test]
    fn prim_spelling_maps_every_primitive_in_each_language() {
        // (prim, rust, go, typescript) — the single source of truth, verified
        // exhaustively here so each target's symbol table only needs to confirm it
        // reads its own column.
        let cases = [
            (Prim::Bool, "bool", "bool", "boolean"),
            (Prim::String, "String", "string", "string"),
            (Prim::Bytes, "Vec<u8>", "[]byte", "Uint8Array"),
            (Prim::I8, "i8", "int8", "number"),
            (Prim::I16, "i16", "int16", "number"),
            (Prim::I32, "i32", "int32", "number"),
            (Prim::I64, "i64", "int64", "bigint"),
            (Prim::U8, "u8", "uint8", "number"),
            (Prim::U16, "u16", "uint16", "number"),
            (Prim::U32, "u32", "uint32", "number"),
            (Prim::U64, "u64", "uint64", "bigint"),
            (Prim::Float, "f64", "float64", "number"),
            (Prim::Timestamp, "Timestamp", "Timestamp", "Timestamp"),
            (Prim::Date, "LocalDate", "LocalDate", "LocalDate"),
            (Prim::Duration, "Duration", "Duration", "Duration"),
            (Prim::Uuid, "Uuid", "Uuid", "Uuid"),
        ];
        for (prim, rust, go, typescript) in cases {
            let s = prim_spelling(&prim);
            assert_eq!(s.rust, rust, "rust {prim:?}");
            assert_eq!(s.go, go, "go {prim:?}");
            assert_eq!(s.typescript, typescript, "typescript {prim:?}");
        }
    }

    #[test]
    fn string_enum_names_a_list_of_verbatim_wire_literals() {
        let decl = string_enum(
            &[("pending".into(), None), ("settled".into(), None)],
            "Status",
        );
        assert!(matches!(decl, Decl::Enum(d)
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"));
    }

    #[test]
    fn emit_shape_dispatches_each_shape_kind_through_its_policy() {
        let field_of = |m: &Member| Field {
            name: Symbol::builtin(m.name.clone()),
            ty: TypeExpr::Ref(Symbol::builtin("x")),
            nullable: false,
            wire: None,
        };
        let mark_enum = |_: &[(String, Option<i64>)], name: &str| {
            vec![Decl::Alias(crate::codegen::tree::Alias {
                name: Symbol::builtin(name.to_string()),
                value: "enum".into(),
            })]
        };
        let mark_union = |_: &str, _: &[Member], name: &str| {
            vec![Decl::Alias(crate::codegen::tree::Alias {
                name: Symbol::builtin(name.to_string()),
                value: "union".into(),
            })]
        };

        // A structure builds an interface of fields via `field_of`.
        let structure = Shape {
            id: "m#Charge".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![Member {
                    name: "amount".into(),
                    target: Tref::Prim(Prim::I64),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                }],
            },
            traits: vec![],
        };
        assert!(matches!(
            &emit_shape(&structure, "rust", field_of, mark_enum, mark_union)[..],
            [Decl::Interface(i)] if i.name.name == "Charge" && i.fields[0].name.name == "amount"
        ));

        // An enum and a union route through their policies, carrying the name.
        let enumeration = Shape {
            id: "m#Status".into(),
            kind: ShapeKind::Enum {
                backing: crate::ir::EnumBacking::String,
                values: vec![("a".into(), None)],
            },
            traits: vec![],
        };
        assert!(matches!(
            &emit_shape(&enumeration, "rust", field_of, mark_enum, mark_union)[..],
            [Decl::Alias(a)] if a.name.name == "Status" && a.value == "enum"
        ));
        let union = Shape {
            id: "m#Method".into(),
            kind: ShapeKind::Union {
                params: vec![],
                discriminator: "type".into(),
                members: vec![],
            },
            traits: vec![],
        };
        assert!(matches!(
            &emit_shape(&union, "rust", field_of, mark_enum, mark_union)[..],
            [Decl::Alias(a)] if a.name.name == "Method" && a.value == "union"
        ));

        // Any other shape kind emits nothing.
        let service = Shape {
            id: "m#Api".into(),
            kind: ShapeKind::Service { operations: vec![] },
            traits: vec![],
        };
        assert!(emit_shape(&service, "rust", field_of, mark_enum, mark_union).is_empty());
    }
}

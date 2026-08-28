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
use crate::codegen::tree::{Decl, EnumDecl, EnumRepr, Field, Interface, TypeExpr};
use crate::ir::{EnumBacking, EnumValue, Member, Prim, Shape, ShapeKind, Tref};

// The core trait vocabulary is read through one module, re-exported here so
// every target keeps reaching it as `conventions::doc_of` and friends.
pub use crate::codegen::traits::{
    core_trait, deprecated_of, doc_of, entries_or_map, has_entries, rename_map, rename_of,
    wire_key, wire_of,
};

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
        // A type parameter is cased like any other type identifier, so a field that
        // applies it matches the definition's parameter clause (both PascalCase).
        Tref::Param(name) => Symbol::builtin(type_case(name)),
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

/// The symbol for a primitive's spelling. A branded well-known type is a
/// declaration of the SDK's shared support group, not of the module that
/// happens to use it, so it carries that import: without it every module would
/// declare its own `Timestamp`, and Go and Rust would treat the two as
/// unrelated named types that do not interconvert.
pub fn prim_symbol(p: &Prim, spelling: &str) -> Symbol {
    match p {
        Prim::Timestamp | Prim::Date | Prim::Duration => {
            Symbol::imported(spelling, crate::codegen::group::ROOT_SUPPORT, spelling)
        }
        _ => Symbol::builtin(spelling),
    }
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
        // `uuid` is not a branded type: it lowers to the native string, like any
        // other string-shaped value.
        Prim::Uuid => ("String", "string", "string"),
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
    emit_enum: impl Fn(&EnumBacking, &[EnumValue], &str, Option<&str>, Option<&str>) -> Vec<Decl>,
    emit_union: impl Fn(&str, &[Member], &str, Option<&str>, Option<&str>) -> Vec<Decl>,
) -> Vec<Decl> {
    let deprecated = deprecated_of(&shape.traits);
    let doc = doc_of(&shape.traits);
    match &shape.kind {
        ShapeKind::Structure { params, members } => vec![Decl::Interface(Interface {
            name: type_name(shape, lang),
            // Type parameters are type identifiers, so they ride the same casing as
            // any other type name; a reference that applies them (a `Param` leaf)
            // goes through the same `type_case`, so the two spellings always match.
            params: params.iter().map(|p| type_case(p)).collect(),
            fields: members.iter().map(&field_of).collect(),
            deprecated,
            doc,
        })],
        ShapeKind::Enum { backing, values } => emit_enum(
            backing,
            values,
            &type_ident(shape, lang),
            deprecated.as_deref(),
            doc.as_deref(),
        ),
        ShapeKind::Union {
            discriminator,
            members,
            ..
        } => emit_union(
            discriminator,
            members,
            &type_ident(shape, lang),
            deprecated.as_deref(),
            doc.as_deref(),
        ),
        _ => vec![],
    }
}

/// An open enum as a named list of its members, carrying the backing
/// representation (string literals or parallel wire integers): the form Go (a
/// named string/int) and TypeScript (a literal union) share. Rust instead
/// hand-writes a `Deserialize` for its `Unknown` arm, so it does not use this. The
/// member names supply the in-code identifiers; their wire form rides the backing.
/// An int-backed value missing a discriminant falls back to zero rather than
/// panicking (the frontend guarantees one is present).
pub fn open_enum(
    backing: &EnumBacking,
    values: &[EnumValue],
    name: &str,
    deprecated: Option<&str>,
    doc: Option<&str>,
) -> Decl {
    let repr = match backing {
        EnumBacking::String => EnumRepr::String,
        EnumBacking::Int => EnumRepr::Int(values.iter().map(|v| v.value.unwrap_or(0)).collect()),
    };
    Decl::Enum(EnumDecl {
        name: Symbol::builtin(name.to_string()),
        members: values
            .iter()
            .map(|v| Symbol::builtin(v.name.clone()))
            .collect(),
        member_docs: values.iter().map(|v| doc_of(&v.traits)).collect(),
        backing: repr,
        deprecated: deprecated.map(str::to_string),
        doc: doc.map(str::to_string),
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
    use crate::ir::Trait;
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

    /// A bagless enum value, for the enum-emission tests.
    fn ev(name: &str, value: Option<i64>) -> EnumValue {
        EnumValue {
            name: name.into(),
            value,
            traits: vec![],
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
    fn deprecated_reads_the_reason_and_the_bare_form() {
        // The fixtures' bare-string value is the reason.
        assert_eq!(
            deprecated_of(&[trait_of("core#deprecated", json!("use v2"))]).as_deref(),
            Some("use v2")
        );
        // The frontend encodes the single argument as a one-element array, and emits
        // the trait id bare (namespace resolution is a later pass).
        assert_eq!(
            deprecated_of(&[trait_of("deprecated", json!(["use v2"]))]).as_deref(),
            Some("use v2")
        );
        // A bare `@deprecated` (no argument) is deprecated without a reason.
        assert_eq!(
            deprecated_of(&[trait_of("deprecated", json!(null))]).as_deref(),
            Some("")
        );
        // Absent means not deprecated.
        assert_eq!(deprecated_of(&[]), None);
    }

    #[test]
    fn doc_reads_the_content_and_ignores_the_empty_form() {
        // The fixtures' bare-string value is the content.
        assert_eq!(
            doc_of(&[trait_of("core#doc", json!("A charge."))]).as_deref(),
            Some("A charge.")
        );
        // The frontend encodes the single argument as a one-element array, with the
        // trait id bare.
        assert_eq!(
            doc_of(&[trait_of("doc", json!(["Multi\nline."]))]).as_deref(),
            Some("Multi\nline.")
        );
        // An empty doc yields None: a comment with no content is not worth emitting.
        assert_eq!(doc_of(&[trait_of("doc", json!([""]))]), None);
        assert_eq!(doc_of(&[trait_of("doc", json!(null))]), None);
        // Absent means no doc.
        assert_eq!(doc_of(&[]), None);
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
    fn emit_shape_cases_generic_params_like_the_leaf_that_applies_them() {
        // The IR carries the parameter in snake/lowercase (`t`); the definition's
        // clause and any field that applies it must both come out PascalCase (`T`),
        // so they always agree.
        let field_of = |m: &Member| Field {
            tag: None,
            name: Symbol::builtin(m.name.clone()),
            ty: type_expr_of(&m.target, &|t: &Tref| {
                leaf_symbol_of(t, |p| Symbol::builtin(format!("{p:?}")), "List", "Map")
            }),
            nullable: false,
            wire: None,
            deprecated: None,
            doc: None,
        };
        let noop_enum =
            |_: &EnumBacking, _: &[EnumValue], _: &str, _: Option<&str>, _: Option<&str>| vec![];
        let noop_union = |_: &str, _: &[Member], _: &str, _: Option<&str>, _: Option<&str>| vec![];
        let shape = Shape {
            id: "m#page".into(),
            kind: ShapeKind::Structure {
                params: vec!["t".into()],
                members: vec![Member {
                    name: "items".into(),
                    target: Tref::List(Box::new(Tref::Param("t".into()))),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                }],
            },
            traits: vec![],
        };
        let decls = emit_shape(&shape, "rust", field_of, noop_enum, noop_union);
        assert!(matches!(&decls[..], [Decl::Interface(i)]
            if i.name.name == "Page"
                && i.params == vec!["T".to_string()]
                && i.fields[0].ty == TypeExpr::list(TypeExpr::Ref(Symbol::builtin("T")))));
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
            (Prim::Uuid, "String", "string", "string"),
        ];
        for (prim, rust, go, typescript) in cases {
            let s = prim_spelling(&prim);
            assert_eq!(s.rust, rust, "rust {prim:?}");
            assert_eq!(s.go, go, "go {prim:?}");
            assert_eq!(s.typescript, typescript, "typescript {prim:?}");
        }
    }

    #[test]
    fn open_enum_names_a_string_backed_list_of_verbatim_wire_literals() {
        let decl = open_enum(
            &EnumBacking::String,
            &[ev("pending", None), ev("settled", None)],
            "Status",
            None,
            None,
        );
        assert!(matches!(decl, Decl::Enum(d)
            if d.name.name == "Status"
                && d.members.len() == 2
                && d.members[0].name == "pending"
                && d.members[1].name == "settled"
                && d.backing == EnumRepr::String));
    }

    #[test]
    fn open_enum_carries_int_wire_integers_parallel_to_members() {
        let decl = open_enum(
            &EnumBacking::Int,
            &[ev("ok", Some(200)), ev("error", Some(500))],
            "HTTPCode",
            None,
            None,
        );
        assert!(matches!(decl, Decl::Enum(d)
            if d.name.name == "HTTPCode"
                && d.members[0].name == "ok"
                && d.members[1].name == "error"
                && d.backing == EnumRepr::Int(vec![200, 500])));
        // A missing discriminant falls back to zero rather than panicking.
        let lenient = open_enum(&EnumBacking::Int, &[ev("ok", None)], "HTTPCode", None, None);
        assert!(matches!(lenient, Decl::Enum(d)
            if d.backing == EnumRepr::Int(vec![0])));
    }

    #[test]
    fn emit_shape_dispatches_each_shape_kind_through_its_policy() {
        let field_of = |m: &Member| Field {
            tag: None,
            name: Symbol::builtin(m.name.clone()),
            ty: TypeExpr::Ref(Symbol::builtin("x")),
            nullable: false,
            wire: None,
            deprecated: None,
            doc: None,
        };
        let mark_enum =
            |_: &EnumBacking, _: &[EnumValue], name: &str, _: Option<&str>, _: Option<&str>| {
                vec![Decl::Alias(crate::codegen::tree::Alias {
                    name: Symbol::builtin(name.to_string()),
                    value: "enum".into(),
                })]
            };
        let mark_union = |_: &str, _: &[Member], name: &str, _: Option<&str>, _: Option<&str>| {
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
                values: vec![ev("a", None)],
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

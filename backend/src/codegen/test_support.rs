//! Shared builders and assertions for codegen unit tests, so every target's
//! tests construct IR shapes and check their symbol table the same way instead
//! of re-declaring the same helpers.
#![cfg(test)]

use crate::codegen::symbol::Symbol;
use crate::codegen::target::Target;
use crate::ir::{EnumBacking, Member, Prim, Shape, ShapeKind, Trait, Tref};

/// A required member with no traits.
pub fn member(name: &str, target: Tref, required: bool) -> Member {
    member_with(name, target, required, vec![])
}

/// A member with explicit traits.
pub fn member_with(name: &str, target: Tref, required: bool, traits: Vec<Trait>) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits,
    }
}

/// A union member: a named arm whose payload is a nominal reference, with an
/// optional `@wire` tag override. Used by the union-codec tests in every target.
pub fn wire_member(name: &str, payload_id: &str, wire: Option<&str>) -> Member {
    let traits = wire
        .map(|w| {
            vec![Trait {
                id: "core#wire".into(),
                value: serde_json::json!(w),
            }]
        })
        .unwrap_or_default();
    member_with(
        name,
        Tref::Ref {
            id: payload_id.into(),
            args: vec![],
        },
        true,
        traits,
    )
}

/// A structure shape with the given members.
pub fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

/// A string-backed enum shape with the given `(wire, discriminant)` values.
pub fn enum_shape(id: &str, values: Vec<(String, Option<i64>)>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Enum {
            backing: EnumBacking::String,
            values,
        },
        traits: vec![],
    }
}

/// A union shape with the given discriminator and variant members.
pub fn union_shape(id: &str, discriminator: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Union {
            params: vec![],
            discriminator: discriminator.into(),
            members,
        },
        traits: vec![],
    }
}

/// Assert a symbol table maps each primitive to the expected in-code name and
/// imports none of them.
pub fn assert_prim_symbols(symbol_of: impl Fn(&Tref) -> Symbol, cases: &[(Prim, &str)]) {
    for (prim, expected) in cases {
        let symbol = symbol_of(&Tref::Prim(prim.clone()));
        assert_eq!(&symbol.name, expected, "{prim:?}");
        assert_eq!(
            symbol.import, None,
            "primitives are not imported ({prim:?})"
        );
    }
}

/// Assert a type parameter is an unimported local name and the collection
/// fallbacks carry the given structural names.
pub fn assert_param_and_collections(
    symbol_of: impl Fn(&Tref) -> Symbol,
    list_name: &str,
    map_name: &str,
) {
    let param = symbol_of(&Tref::Param("T".into()));
    assert_eq!(param.name, "T");
    assert_eq!(param.import, None);
    assert_eq!(
        symbol_of(&Tref::List(Box::new(Tref::Prim(Prim::Bool)))).name,
        list_name
    );
    assert_eq!(
        symbol_of(&Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::Bool)),
        ))
        .name,
        map_name
    );
}

/// Assert a target emits nothing for an operation stub and ignores the opaque
/// wire descriptor.
pub fn assert_emits_no_op_stub(target: &impl Target) {
    let op = Shape {
        id: "billing#Create".into(),
        kind: ShapeKind::Operation {
            input: None,
            output: None,
            errors: vec![],
        },
        traits: vec![],
    };
    assert!(target
        .emit_op_stub(&op, &serde_json::json!({"http_method": "POST"}))
        .is_empty());
}

//! Shared builders and assertions for codegen unit tests, so every target's
//! tests construct IR shapes and check their symbol table the same way instead
//! of re-declaring the same helpers.
#![cfg(test)]

use crate::codegen::symbol::Symbol;
use crate::codegen::target::{RenderRules, Target};
use crate::codegen::tree::Decl;
use crate::ir::{EnumBacking, Member, Module, Prim, Shape, ShapeKind, Trait, Tref};

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

/// An int-backed enum shape with the given `(wire, discriminant)` values.
pub fn int_enum_shape(id: &str, values: Vec<(String, Option<i64>)>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Enum {
            backing: EnumBacking::Int,
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

/// A bare trait with the given id and JSON value.
pub fn trait_of(id: &str, value: serde_json::Value) -> Trait {
    Trait {
        id: id.into(),
        value,
    }
}

/// An error shape carrying its discrimination traits: the HTTP status, an
/// optional body code, and retryability.
pub fn error_shape(
    id: &str,
    members: Vec<Member>,
    status: i64,
    code: Option<&str>,
    retryable: bool,
) -> Shape {
    let mut shape = structure(id, members);
    shape
        .traits
        .push(trait_of("status", serde_json::json!([status])));
    if let Some(code) = code {
        shape
            .traits
            .push(trait_of("errorCode", serde_json::json!([code])));
    }
    if retryable {
        shape
            .traits
            .push(trait_of("retryable", serde_json::Value::Null));
    }
    shape
}

/// An operation from `m#charge_input` to `m#charge` with the given traits and
/// declared-error references.
pub fn operation(id: &str, traits: Vec<Trait>, errors: Vec<&str>) -> Shape {
    let reference = |id: &str| Tref::Ref {
        id: id.into(),
        args: vec![],
    };
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input: Some(reference("m#charge_input")),
            output: Some(reference("m#charge")),
            errors: errors.into_iter().map(reference).collect(),
        },
        traits,
    }
}

/// The shared error-surface fixture: one async transport operation declaring a
/// retryable, coded 402 error and a codeless 429 error, so every target's
/// tests exercise the same taxonomy, client, and discrimination inputs.
pub fn error_demo_module() -> Module {
    Module {
        name: "m".into(),
        shapes: vec![
            structure(
                "m#charge",
                vec![member("id", Tref::Prim(Prim::String), true)],
            ),
            structure(
                "m#charge_input",
                vec![member("amount", Tref::Prim(Prim::I64), true)],
            ),
            error_shape(
                "m#payment_declined",
                vec![member("message", Tref::Prim(Prim::String), true)],
                402,
                Some("payment_declined"),
                true,
            ),
            error_shape("m#rate_limited", vec![], 429, None, false),
        ],
        operations: vec![operation(
            "m#create_charge",
            vec![trait_of(
                "http",
                serde_json::json!({"method": "POST", "path": "/charges"}),
            )],
            vec!["m#payment_declined", "m#rate_limited"],
        )],
    }
}

/// Render declarations through a target's render rules, joined by newlines.
pub fn rendered(decls: &[Decl], rules: &impl RenderRules) -> String {
    decls
        .iter()
        .map(|d| rules.render_decl(d))
        .collect::<Vec<_>>()
        .join("\n")
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

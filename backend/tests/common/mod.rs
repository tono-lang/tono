//! Shared IR builders and the canonical wire fixture for the codegen integration
//! harnesses (the per-language round-trips and the cross-language conformance
//! check), so each does not re-declare the same module.
//!
//! Each harness is a separate test binary that uses a different subset of these
//! helpers, so unused items per binary are expected.
#![allow(dead_code)]

use tono_backend::ir::{EnumBacking, Member, Module, Prim, Shape, ShapeKind, Trait, Tref};

/// A member with explicit traits.
pub fn member(name: &str, target: Tref, required: bool, traits: Vec<Trait>) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits,
    }
}

/// A nominal type reference with no generic arguments.
pub fn reference(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
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

/// The shared module exercised by every harness: the full wire matrix in one
/// shape set — a 64-bit integer, bytes, an optional 64-bit integer, an open
/// enum, an internally-tagged union, and an `@entries` pairs-array map.
pub fn matrix_module() -> Module {
    let entries = vec![Trait {
        id: "core#entries".into(),
        value: serde_json::json!(true),
    }];
    Module {
        name: "models".into(),
        shapes: vec![
            structure(
                "models#Account",
                vec![
                    member("account_id", Tref::Prim(Prim::I64), true, vec![]),
                    member("secret", Tref::Prim(Prim::Bytes), true, vec![]),
                    member("tip", Tref::Prim(Prim::I64), false, vec![]),
                    member("status", reference("models#Status"), true, vec![]),
                    member("code", reference("models#http_code"), true, vec![]),
                    member("method", reference("models#Method"), true, vec![]),
                    member(
                        "counts",
                        Tref::Map(
                            Box::new(Tref::Prim(Prim::I32)),
                            Box::new(Tref::Prim(Prim::String)),
                        ),
                        true,
                        entries,
                    ),
                ],
            ),
            Shape {
                id: "models#Status".into(),
                kind: ShapeKind::Enum {
                    backing: EnumBacking::String,
                    values: vec![("active".into(), None), ("closed".into(), None)],
                },
                traits: vec![],
            },
            Shape {
                id: "models#http_code".into(),
                kind: ShapeKind::Enum {
                    backing: EnumBacking::Int,
                    values: vec![
                        ("ok".into(), Some(200)),
                        ("not_found".into(), Some(404)),
                        ("error".into(), Some(500)),
                    ],
                },
                traits: vec![],
            },
            Shape {
                id: "models#Method".into(),
                kind: ShapeKind::Union {
                    params: vec![],
                    discriminator: "type".into(),
                    members: vec![
                        member("card", reference("models#card_data"), true, vec![]),
                        member("bank", reference("models#bank_data"), true, vec![]),
                    ],
                },
                traits: vec![],
            },
            structure(
                "models#card_data",
                vec![member("last4", Tref::Prim(Prim::String), true, vec![])],
            ),
            structure(
                "models#bank_data",
                vec![member("iban", Tref::Prim(Prim::String), true, vec![])],
            ),
            declared_error(
                "models#payment_declined",
                vec![member("message", Tref::Prim(Prim::String), true, vec![])],
                402,
                Some("payment_declined"),
                true,
            ),
            declared_error("models#rate_limited", vec![], 429, None, false),
        ],
        // One async operation carrying both declared errors, so every harness
        // exercises the generated error surface, the client, and the
        // per-operation discriminator alongside the wire matrix.
        operations: vec![create_charge_operation()],
    }
}

/// The async operation the harnesses exercise: `Account` to `Account` with a
/// transport binding and both declared errors.
pub fn create_charge_operation() -> Shape {
    Shape {
        id: "models#create_charge".into(),
        kind: ShapeKind::Operation {
            input: Some(reference("models#Account")),
            output: Some(reference("models#Account")),
            errors: vec![
                reference("models#payment_declined"),
                reference("models#rate_limited"),
            ],
        },
        traits: vec![Trait {
            id: "core#http".into(),
            value: serde_json::json!({"method": "POST", "path": "/charges"}),
        }],
    }
}

/// An error shape carrying its discrimination traits: the HTTP status, an
/// optional body code, and retryability.
pub fn declared_error(
    id: &str,
    members: Vec<Member>,
    status: i64,
    code: Option<&str>,
    retryable: bool,
) -> Shape {
    let mut shape = structure(id, members);
    shape.traits.push(Trait {
        id: "core#status".into(),
        value: serde_json::json!([status]),
    });
    if let Some(code) = code {
        shape.traits.push(Trait {
            id: "core#errorCode".into(),
            value: serde_json::json!([code]),
        });
    }
    if retryable {
        shape.traits.push(Trait {
            id: "core#retryable".into(),
            value: serde_json::Value::Null,
        });
    }
    shape
}

/// The canonical wire document for the shared module: exercises i64-as-string,
/// bytes-as-base64, an optional i64, a string-backed open-enum value, an
/// int-backed open-enum value (the bare integer `200`), an internally-tagged
/// union, and the `@entries` pairs array.
pub const CANONICAL_WIRE: &str = concat!(
    "{",
    "\"account_id\":\"9007199254740993\",",
    "\"secret\":\"AQID/g==\",",
    "\"tip\":\"500\",",
    "\"status\":\"active\",",
    "\"code\":200,",
    "\"method\":{\"type\":\"card\",\"last4\":\"4242\"},",
    "\"counts\":[[7,\"a\"],[3,\"b\"]]",
    "}"
);

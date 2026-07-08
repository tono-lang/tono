//! Property tests for the IR mirror: an arbitrary well-formed model survives the
//! full text round-trip as JSON *data*, and re-encoding is idempotent. This is
//! the Rust half of the round-trip property the frontend checks with QCheck; it
//! exercises the hand-written `Tref` codec, the double-option default, and the
//! flattened internally-tagged shape kinds under randomized input rather than the
//! fixed golden fixtures.

use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;
use serde_json::Value;
use tono_backend::ir::{
    check_roundtrip, decode_model, Constraint, EnumBacking, Member, Model, Module, Prim, Shape,
    ShapeKind, Trait, Tref, TONO_IR_VERSION,
};

// Small fixed pools keep generated documents well-formed and stable while still
// exercising every variant, mirroring the frontend's QCheck generators.
fn ident() -> impl Strategy<Value = String> {
    select(vec!["a#A", "a#B", "b#C", "core#Page", "core#List"]).prop_map(String::from)
}
fn param() -> impl Strategy<Value = String> {
    select(vec!["T", "U", "K", "V"]).prop_map(String::from)
}
fn name() -> impl Strategy<Value = String> {
    select(vec!["id", "amount", "note", "items", "total", "kind"]).prop_map(String::from)
}
fn modname() -> impl Strategy<Value = String> {
    select(vec!["payments", "core", "lab"]).prop_map(String::from)
}
// Finite floats whose canonical text is stable through a JSON round-trip.
fn finite_float() -> impl Strategy<Value = f64> {
    select(vec![0.0f64, 1.0, 2.5, 10.0, 100.0, -1.0, 0.25])
}

fn prim() -> impl Strategy<Value = Prim> {
    prop_oneof![
        Just(Prim::Bool),
        Just(Prim::String),
        Just(Prim::Bytes),
        Just(Prim::I8),
        Just(Prim::I16),
        Just(Prim::I32),
        Just(Prim::I64),
        Just(Prim::U8),
        Just(Prim::U16),
        Just(Prim::U32),
        Just(Prim::U64),
        Just(Prim::Float),
        Just(Prim::Timestamp),
        Just(Prim::Date),
        Just(Prim::Duration),
        Just(Prim::Uuid),
    ]
}

fn tref() -> impl Strategy<Value = Tref> {
    let leaf = prop_oneof![
        prim().prop_map(Tref::Prim),
        param().prop_map(Tref::Param),
        ident().prop_map(|id| Tref::Ref { id, args: vec![] }),
    ];
    // depth 3, up to 16 nodes, up to 2 children per interior node.
    leaf.prop_recursive(3, 16, 2, |inner| {
        prop_oneof![
            (ident(), vec(inner.clone(), 0..2)).prop_map(|(id, args)| Tref::Ref { id, args }),
            inner.clone().prop_map(|t| Tref::List(Box::new(t))),
            (inner.clone(), inner).prop_map(|(k, v)| Tref::Map(Box::new(k), Box::new(v))),
        ]
    })
}

// Arbitrary JSON for defaults and trait values. Numbers stay integral so the
// property targets IR structure, not float-text formatting.
fn json() -> impl Strategy<Value = Value> {
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        (-1000i64..1000).prop_map(Value::from),
        select(vec!["x", "y", "", "hello"]).prop_map(Value::from),
    ];
    leaf.prop_recursive(2, 12, 3, |inner| {
        prop_oneof![
            vec(inner.clone(), 0..3).prop_map(Value::from),
            vec(
                (
                    select(vec!["a", "b", "c", "k"]).prop_map(String::from),
                    inner
                ),
                0..3,
            )
            .prop_map(|kvs| Value::Object(kvs.into_iter().collect())),
        ]
    })
}

fn constraint() -> impl Strategy<Value = Constraint> {
    prop_oneof![
        (
            option::of(finite_float()),
            option::of(finite_float()),
            any::<bool>(),
            any::<bool>(),
        )
            .prop_map(|(min, max, excl_min, excl_max)| Constraint::Range {
                min,
                max,
                excl_min,
                excl_max,
            }),
        (option::of(0i64..1000), option::of(0i64..1000))
            .prop_map(|(min, max)| Constraint::Length { min, max }),
        select(vec!["^[a-z]+$", ".*", "[0-9]{3}"]).prop_map(|s| Constraint::Pattern(s.into())),
        select(vec![0.5f64, 1.0, 2.0, 0.25]).prop_map(Constraint::MultipleOf),
    ]
}

fn trait_() -> impl Strategy<Value = Trait> {
    (ident(), json()).prop_map(|(id, value)| Trait { id, value })
}

fn member() -> impl Strategy<Value = Member> {
    (
        name(),
        tref(),
        any::<bool>(),
        option::of(json()),
        vec(constraint(), 0..2),
        vec(trait_(), 0..2),
    )
        .prop_map(
            |(name, target, required, default, constraints, traits)| Member {
                name,
                target,
                required,
                // An outer `Some` marks a present default; `None` leaves it absent.
                default: default.map(Some),
                constraints,
                traits,
            },
        )
}

fn shape_kind() -> impl Strategy<Value = ShapeKind> {
    prop_oneof![
        (vec(param(), 0..2), vec(member(), 0..3))
            .prop_map(|(params, members)| ShapeKind::Structure { params, members }),
        (
            vec(param(), 0..2),
            vec(member(), 0..3),
            select(vec!["type", "kind", "@type"]),
        )
            .prop_map(|(params, members, d)| ShapeKind::Union {
                params,
                members,
                discriminator: d.into(),
            }),
        (
            select(vec![EnumBacking::String, EnumBacking::Int]),
            vec(
                (
                    select(vec!["a", "b", "c"]).prop_map(String::from),
                    option::of(0i64..100)
                ),
                0..3,
            ),
        )
            .prop_map(|(backing, values)| ShapeKind::Enum { backing, values }),
        vec(ident(), 0..3).prop_map(|operations| ShapeKind::Service { operations }),
        (option::of(tref()), option::of(tref()), vec(tref(), 0..2)).prop_map(
            |(input, output, errors)| ShapeKind::Operation {
                input,
                output,
                errors,
            }
        ),
    ]
}

fn shape() -> impl Strategy<Value = Shape> {
    (ident(), shape_kind(), vec(trait_(), 0..2)).prop_map(|(id, kind, traits)| Shape {
        id,
        kind,
        traits,
    })
}

fn module() -> impl Strategy<Value = Module> {
    (modname(), vec(shape(), 0..3), vec(shape(), 0..2)).prop_map(|(name, shapes, operations)| {
        Module {
            name,
            shapes,
            operations,
            extensions: vec![],
        }
    })
}

fn model() -> impl Strategy<Value = Model> {
    vec(module(), 0..2).prop_map(|modules| Model {
        tono_ir_version: TONO_IR_VERSION,
        modules,
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// `decode(encode(m))` re-encodes to the same JSON data: the property oracle
    /// needs no reference implementation, only the relation itself.
    #[test]
    fn model_survives_text_roundtrip(m in model()) {
        let encoded = serde_json::to_string(&m).expect("encode");
        let rt = check_roundtrip(&encoded).expect("decode");
        prop_assert!(
            rt.equal,
            "round-trip diverged:\n  original:  {}\n  reencoded: {}",
            rt.original,
            rt.reencoded
        );
    }

    /// Re-encoding is a fixed point: encoding a decoded document yields the same
    /// data as the first encoding (idempotence).
    #[test]
    fn reencode_is_idempotent(m in model()) {
        let once = serde_json::to_string(&m).expect("encode");
        let decoded = decode_model(&once).expect("decode");
        let twice = serde_json::to_string(&decoded).expect("re-encode");
        let a: Value = serde_json::from_str(&once).expect("parse first");
        let b: Value = serde_json::from_str(&twice).expect("parse second");
        prop_assert_eq!(a, b);
    }
}

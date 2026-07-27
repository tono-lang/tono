//! The compat surface: what the layout routes to an internal group never entered
//! it, so changing such a declaration is not a change a consumer can observe.
//!
//! Separate from the rest of the compat tests so each file stays within the repo
//! size gate.

use serde_json::json;
use tono_backend::compat::diff;
use tono_backend::ir::{Member, Model, Module, Prim, Shape, ShapeKind, Trait, Tref};

fn model(shapes: Vec<Shape>) -> Model {
    Model {
        tono_ir_version: 6,
        modules: vec![Module {
            name: "billing".into(),
            extensions: vec![],
            shapes,
            operations: vec![],
        }],
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

/// Mark a shape public, the way the frontend lowers `pub`.
fn public(mut shape: Shape) -> Shape {
    shape.traits.push(Trait {
        id: "pub".into(),
        value: json!(null),
    });
    shape
}

#[test]
fn a_change_to_a_shape_the_sdk_never_exposed_is_not_a_change() {
    // `Helper` is unmarked and nothing public reaches it, so the layout routes
    // it to the module's internal group: no consumer can name it, and retyping
    // its member cannot break one.
    let before = model(vec![
        public(structure(
            "billing#Charge",
            vec![member("amount", Tref::Prim(Prim::I64), true)],
        )),
        structure(
            "billing#Helper",
            vec![member("scratch", Tref::Prim(Prim::I64), true)],
        ),
    ]);
    let after = model(vec![
        public(structure(
            "billing#Charge",
            vec![member("amount", Tref::Prim(Prim::I64), true)],
        )),
        structure(
            "billing#Helper",
            vec![member("scratch", Tref::Prim(Prim::String), true)],
        ),
    ]);
    assert!(diff(&before, &after).changes.is_empty());
}

/// Visibility is opt-in, so forgetting the marker on a shape that had it drops
/// that shape from the SDK's surface. This is what catches it: the shape leaves
/// the compared surface, which reads as a removal, and the gate fails on it.
#[test]
fn a_shape_leaving_or_joining_the_surface_is_still_a_change() {
    let hidden = model(vec![
        public(structure(
            "billing#Charge",
            vec![member("amount", Tref::Prim(Prim::I64), true)],
        )),
        structure(
            "billing#Note",
            vec![member("body", Tref::Prim(Prim::String), true)],
        ),
    ]);
    let exposed = model(vec![
        public(structure(
            "billing#Charge",
            vec![member("amount", Tref::Prim(Prim::I64), true)],
        )),
        public(structure(
            "billing#Note",
            vec![member("body", Tref::Prim(Prim::String), true)],
        )),
    ]);
    // Joining the surface is additive; leaving it takes a name a consumer could
    // have been using.
    assert!(!diff(&hidden, &exposed).changes.is_empty());
    let leaving = diff(&exposed, &hidden);
    assert!(leaving
        .changes
        .iter()
        .any(|c| c.key.contains("billing#Note")));
}

//! Shared builders for codegen unit tests, so every target's tests construct IR
//! shapes the same way instead of re-declaring the same helpers.
#![cfg(test)]

use crate::ir::{Member, Shape, ShapeKind, Trait, Tref};

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

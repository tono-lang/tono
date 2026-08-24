//! The Go expression of a declared test's pinned construction value. The
//! frontend carries the value as wire-form JSON, verbatim; a scalar keeps
//! the `@default` spelling ([`super::super::literal`]), but the JSON spelling
//! of a collection or a structure (`[1.0, 2.0]`, `{"xs": [1]}`) is not Go,
//! so those become composite literals typed by the field they land in,
//! recursing on the element or member types.

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::{field_ident, wire_key};
use crate::ir::{Member, Module, Prim, ShapeKind, Tref};

use super::super::{go_type, literal, LANG};

/// The Go literal of a pinned value in the position of type `t`.
pub(super) fn pinned_literal(
    module: &Module,
    config: &CasingConfig,
    t: &Tref,
    v: &serde_json::Value,
) -> String {
    match (t, v) {
        (Tref::List(inner), serde_json::Value::Array(items)) => {
            let items: Vec<String> = items
                .iter()
                .map(|item| pinned_literal(module, config, inner, item))
                .collect();
            format!("{}{{{}}}", go_type(t), items.join(", "))
        }
        (Tref::Map(_, value), serde_json::Value::Object(pairs)) => {
            let pairs: Vec<String> = pairs
                .iter()
                .map(|(key, item)| {
                    format!("{key:?}: {}", pinned_literal(module, config, value, item))
                })
                .collect();
            format!("{}{{{}}}", go_type(t), pairs.join(", "))
        }
        (Tref::Ref { id, .. }, serde_json::Value::Object(fields)) => {
            match structure_members(module, id) {
                Some(members) => struct_literal(module, config, t, members, fields),
                None => literal(t, v),
            }
        }
        _ => literal(t, v),
    }
}

/// The Go zero value of an unpinned `@arg`'s type: an untyped `0`/`false`
/// converts to any numeric or boolean parameter, a collection is `nil`, a
/// structure is its empty literal, and the string-backed types keep their
/// empty-string conversion.
pub(super) fn zero_literal(module: &Module, t: &Tref) -> String {
    match t {
        Tref::List(_) | Tref::Map(_, _) => "nil".to_string(),
        Tref::Prim(Prim::Bool) => "false".to_string(),
        Tref::Prim(
            Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64
            | Prim::Float,
        ) => "0".to_string(),
        Tref::Ref { id, .. } if structure_members(module, id).is_some() => {
            format!("{}{{}}", go_type(t))
        }
        _ => literal(t, &serde_json::Value::String(String::new())),
    }
}

/// `Type{Field: value, ...}`: the members the pinned object names, matched
/// by wire key (the JSON the frontend carries is wire-form), spelled under
/// their Go field names. An optional scalar member is a pointer field, and
/// Go cannot take the address of a literal: a structure literal is
/// addressable (`&T{..}`), anything else goes through a closure that binds
/// it first.
fn struct_literal(
    module: &Module,
    config: &CasingConfig,
    t: &Tref,
    members: &[Member],
    fields: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let parts: Vec<String> = members
        .iter()
        .filter_map(|member| {
            let value = fields.get(&wire_key(member))?;
            let mut lit = pinned_literal(module, config, &member.target, value);
            let collection = matches!(member.target, Tref::List(_) | Tref::Map(_, _));
            if !member.required && !collection {
                lit = match &member.target {
                    Tref::Ref { id, .. } if structure_members(module, id).is_some() => {
                        format!("&{lit}")
                    }
                    inner => format!("func() *{} {{ v := {lit}; return &v }}()", go_type(inner)),
                };
            }
            Some(format!("{}: {lit}", field_ident(member, config, LANG)))
        })
        .collect();
    format!("{}{{{}}}", go_type(t), parts.join(", "))
}

fn structure_members<'m>(module: &'m Module, id: &str) -> Option<&'m [Member]> {
    module
        .shapes
        .iter()
        .find(|s| s.id == id)
        .and_then(|shape| match &shape.kind {
            ShapeKind::Structure { members, .. } => Some(members.as_slice()),
            _ => None,
        })
}

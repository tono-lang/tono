//! The Go target: maps the IR to idiomatic Go with `encoding/json` struct tags
//! doing the wire work, plus a thin codec layer only where the standard library
//! cannot. A 64-bit integer rides a `,string` tag, `bytes` is base64 natively, the
//! open enum and the well-known types are named strings serialized natively, and an
//! optional field is a pointer with `,omitempty`. Without a sum type, a union
//! becomes an interface plus one wrapper struct per variant (each with a
//! `MarshalJSON`) and a free `unmarshalX`; a struct holding a union field gets a
//! thin `UnmarshalJSON`. The `@entries` map is a generic `Entries[K, V]`.

pub mod client;
pub mod codecs;
pub mod emit;
pub mod entry;
pub mod errors;
pub mod render;
pub mod symbols;
pub mod types;

pub use render::{GoRules, SlotRules};

crate::declare_target! {
    /// The Go target: the Symbol table and emitters. Render rules live in
    /// [`GoRules`]; the engine supplies the tree, import collection, casing, and
    /// the formatter.
    pub struct GoTarget => {
        name: "go",
        symbol_of: symbols::symbol_of,
        emit_type: types::emit_type,
        casing: types::go_casing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::Target;
    use crate::ir::{Prim, Tref};

    #[test]
    fn target_identity() {
        assert_eq!(GoTarget.name(), "go");
    }

    #[test]
    fn symbol_of_delegates_to_the_symbol_table() {
        assert_eq!(GoTarget.symbol_of(&Tref::Prim(Prim::I64)).name, "int64");
    }
}

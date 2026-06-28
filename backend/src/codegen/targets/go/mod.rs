//! The Go target: maps the IR to idiomatic Go. Without a sum type, unions become
//! a struct with one pointer per variant and hand-written JSON methods; 64-bit
//! integers ride the json `,string` tag, `bytes` is `[]byte` (base64 by
//! `encoding/json`), the open enum is a named string, and well-known types are
//! named strings.

pub mod codecs;
pub mod emit;
pub mod render;
pub mod symbols;
pub mod types;

pub use render::GoRules;

crate::declare_target! {
    /// The Go target: the Symbol table and emitters. Render rules live in
    /// [`GoRules`]; the engine supplies the tree, import collection, casing, and
    /// the formatter.
    pub struct GoTarget => {
        name: "go",
        symbol_of: symbols::symbol_of,
        emit_type: types::emit_type,
        casing: types::go_casing,
        runtime_pkg: "sdk-http-runtime-go",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::Target;
    use crate::codegen::test_support::assert_emits_no_op_stub;
    use crate::ir::{Prim, Tref};

    #[test]
    fn target_identity_and_runtime() {
        assert_eq!(GoTarget.name(), "go");
        assert_eq!(GoTarget.runtime_pkg(), "sdk-http-runtime-go");
    }

    #[test]
    fn symbol_of_delegates_to_the_symbol_table() {
        assert_eq!(GoTarget.symbol_of(&Tref::Prim(Prim::I64)).name, "int64");
    }

    #[test]
    fn emit_op_stub_emits_nothing_and_ignores_the_descriptor() {
        assert_emits_no_op_stub(&GoTarget);
    }
}

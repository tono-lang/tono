//! The TypeScript target: maps the IR to idiomatic TypeScript with correct wire
//! encoding for the hard cases (open enum, internally-tagged union, generics,
//! nullable, i64-as-string, bytes-as-base64, branded well-known types).

pub mod codecs;
pub mod emit;
pub mod errors;
pub mod render;
pub mod symbols;
pub mod types;

pub use render::TsRules;

crate::declare_target! {
    /// The TypeScript target: the Symbol table and emitters. Render rules live in
    /// [`TsRules`]; the engine supplies the tree, import collection, casing, and
    /// the formatter.
    pub struct TsTarget => {
        name: "typescript",
        symbol_of: symbols::symbol_of,
        emit_type: types::emit_type,
        casing: types::ts_casing,
        runtime_pkg: "@sdk/http-runtime-ts",
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
        assert_eq!(TsTarget.name(), "typescript");
        assert_eq!(TsTarget.runtime_pkg(), "@sdk/http-runtime-ts");
    }

    #[test]
    fn symbol_of_delegates_to_the_symbol_table() {
        assert_eq!(TsTarget.symbol_of(&Tref::Prim(Prim::I64)).name, "bigint");
    }

    #[test]
    fn emit_op_stub_emits_nothing_and_ignores_the_descriptor() {
        assert_emits_no_op_stub(&TsTarget);
    }
}

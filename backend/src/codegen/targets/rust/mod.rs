//! The Rust target: maps the IR to idiomatic Rust with serde-driven wire
//! encoding for the hard cases (open enum, internally-tagged union, generics,
//! nullable, i64-as-string, bytes-as-base64, branded well-known types).
//!
//! Unlike the TypeScript target, which emits explicit `encode`/`decode` codec
//! functions, Rust leans on serde derives and attributes; only the few cases
//! serde cannot express idiomatically (the open-enum `Unknown` arm, the
//! integer-as-string helpers) are emitted as verbatim items by a later phase.

pub mod codecs;
pub mod emit;
pub mod errors;
pub mod render;
pub mod symbols;
pub mod types;

pub use render::RustRules;

crate::declare_target! {
    /// The Rust target: the Symbol table and emitters. Render rules and codec
    /// helpers live in sibling modules; the engine supplies the tree, import
    /// collection, casing, and the formatter.
    pub struct RustTarget => {
        name: "rust",
        symbol_of: symbols::symbol_of,
        emit_type: types::emit_type,
        casing: types::rust_casing,
        runtime_pkg: "sdk-http-runtime-rs",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::Target;
    use crate::codegen::test_support::{assert_emits_no_op_stub, member, structure};
    use crate::ir::{Prim, Tref};

    #[test]
    fn target_identity_and_runtime() {
        assert_eq!(RustTarget.name(), "rust");
        assert_eq!(RustTarget.runtime_pkg(), "sdk-http-runtime-rs");
    }

    #[test]
    fn symbol_of_delegates_to_the_symbol_table() {
        assert_eq!(RustTarget.symbol_of(&Tref::Prim(Prim::I64)).name, "i64");
    }

    #[test]
    fn emit_op_stub_emits_nothing_and_ignores_the_descriptor() {
        assert_emits_no_op_stub(&RustTarget);
    }

    #[test]
    fn emit_type_maps_a_structure_to_a_struct_interface() {
        let shape = structure(
            "billing#Charge",
            vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
        );
        let decls = RustTarget.emit_type(&shape);
        assert!(
            matches!(&decls[..], [crate::codegen::tree::Decl::Interface(i)]
            if i.name.name == "Charge" && i.fields[0].name.name == "amount_cents")
        );
    }
}

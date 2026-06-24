//! The TypeScript target: maps the IR to idiomatic TypeScript with correct wire
//! encoding for the hard cases (open enum, internally-tagged union, generics,
//! nullable, i64-as-string, bytes-as-base64, branded well-known types).

pub mod codecs;
pub mod emit;
pub mod render;
pub mod symbols;
pub mod types;

use serde_json::Value;

use crate::codegen::symbol::Symbol;
use crate::codegen::target::{Fragment, Target};
use crate::ir::{Shape, Tref};

pub use render::TsRules;

/// The TypeScript target: the Symbol table and emitters. Render rules live in
/// [`TsRules`]; the engine supplies the tree, import collection, casing, and the
/// formatter.
pub struct TsTarget;

impl Target for TsTarget {
    fn name(&self) -> &str {
        "typescript"
    }

    fn symbol_of(&self, t: &Tref) -> Symbol {
        symbols::symbol_of(t)
    }

    fn emit_type(&self, shape: &Shape) -> Fragment {
        types::emit_type(shape, &types::ts_casing())
    }

    fn emit_op_stub(&self, _op: &Shape, _descriptor: &Value) -> Fragment {
        // Operation stubs (signature + embedded descriptor + runtime.execute) are
        // owned by the protocol/runtime work; this target emits none yet.
        Vec::new()
    }

    fn runtime_pkg(&self) -> &str {
        "@sdk/http-runtime-ts"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Prim, ShapeKind};
    use serde_json::json;

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
        let op = Shape {
            id: "billing#Create".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![],
        };
        assert!(TsTarget
            .emit_op_stub(&op, &json!({"http_method": "POST"}))
            .is_empty());
    }
}

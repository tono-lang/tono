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

use serde_json::Value;

use crate::codegen::symbol::Symbol;
use crate::codegen::target::{Fragment, Target};
use crate::ir::{Shape, Tref};

pub use render::GoRules;

/// The Go target: the Symbol table and emitters. Render rules live in [`GoRules`];
/// the engine supplies the tree, import collection, casing, and the formatter.
pub struct GoTarget;

impl Target for GoTarget {
    fn name(&self) -> &str {
        "go"
    }

    fn symbol_of(&self, t: &Tref) -> Symbol {
        symbols::symbol_of(t)
    }

    fn emit_type(&self, shape: &Shape) -> Fragment {
        types::emit_type(shape, &types::go_casing())
    }

    fn emit_op_stub(&self, _op: &Shape, _descriptor: &Value) -> Fragment {
        // Operation stubs (signature + embedded descriptor + runtime.execute) are
        // owned by the protocol/runtime work; this target emits none yet.
        Vec::new()
    }

    fn runtime_pkg(&self) -> &str {
        "sdk-http-runtime-go"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Prim, ShapeKind};
    use serde_json::json;

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
        let op = Shape {
            id: "billing#Create".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![],
        };
        assert!(GoTarget
            .emit_op_stub(&op, &json!({"http_method": "POST"}))
            .is_empty());
    }
}

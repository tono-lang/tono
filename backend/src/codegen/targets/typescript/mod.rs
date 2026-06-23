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
    use crate::codegen::render::render_file;
    use crate::codegen::tree::File;
    use crate::codegen::Formatter;
    use crate::ir::{Member, Prim, ShapeKind};
    use serde_json::json;

    fn passthrough() -> Formatter {
        // `cat` echoes the rough text unchanged, so tests assert the engine's
        // assembled output without depending on prettier being installed.
        Formatter::new("cat", vec![])
    }

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

    #[test]
    fn a_structure_renders_to_a_typescript_interface_end_to_end() {
        let shape = Shape {
            id: "billing#Charge".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![
                    Member {
                        name: "amount_cents".into(),
                        target: Tref::Prim(Prim::I64),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                    Member {
                        name: "customer".into(),
                        target: Tref::Ref {
                            id: "crm#Customer".into(),
                            args: vec![],
                        },
                        required: false,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                ],
            },
            traits: vec![],
        };
        let file = File {
            module: "billing".into(),
            decls: TsTarget.emit_type(&shape),
        };
        let out = render_file(&file, &TsRules, &passthrough()).text;
        assert!(out.contains("import { Customer } from \"./crm\";"));
        assert!(out.contains("export interface Charge {"));
        assert!(out.contains("  amountCents: bigint;"));
        assert!(out.contains("  customer?: Customer | null;"));
    }
}

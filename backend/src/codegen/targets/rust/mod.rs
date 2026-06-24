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
pub mod render;
pub mod symbols;
pub mod types;

use serde_json::Value;

use crate::codegen::symbol::Symbol;
use crate::codegen::target::{Fragment, Target};
use crate::ir::{Shape, Tref};

pub use render::RustRules;

/// The Rust target: the Symbol table and emitters. Render rules and codec helpers
/// live in later modules; the engine supplies the tree, import collection,
/// casing, and the formatter.
pub struct RustTarget;

impl Target for RustTarget {
    fn name(&self) -> &str {
        "rust"
    }

    fn symbol_of(&self, t: &Tref) -> Symbol {
        symbols::symbol_of(t)
    }

    fn emit_type(&self, shape: &Shape) -> Fragment {
        types::emit_type(shape, &types::rust_casing())
    }

    fn emit_op_stub(&self, _op: &Shape, _descriptor: &Value) -> Fragment {
        // Operation stubs (signature + embedded descriptor + runtime.execute) are
        // owned by the protocol/runtime work; this target emits none yet.
        Vec::new()
    }

    fn runtime_pkg(&self) -> &str {
        "sdk-http-runtime-rs"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Member, Prim, ShapeKind};
    use serde_json::json;

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
        let op = Shape {
            id: "billing#Create".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![],
        };
        assert!(RustTarget
            .emit_op_stub(&op, &json!({"http_method": "POST"}))
            .is_empty());
    }

    #[test]
    fn emit_type_maps_a_structure_to_a_struct_interface() {
        let shape = Shape {
            id: "billing#Charge".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![Member {
                    name: "amount_cents".into(),
                    target: Tref::Prim(Prim::I64),
                    required: true,
                    default: None,
                    constraints: vec![],
                    traits: vec![],
                }],
            },
            traits: vec![],
        };
        let decls = RustTarget.emit_type(&shape);
        assert!(
            matches!(&decls[..], [crate::codegen::tree::Decl::Interface(i)]
            if i.name.name == "Charge" && i.fields[0].name.name == "amount_cents")
        );
    }

    #[test]
    fn a_structure_renders_to_a_rust_struct_end_to_end() {
        use crate::codegen::render::render_file;
        use crate::codegen::tree::File;
        use crate::codegen::Formatter;

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
            decls: RustTarget.emit_type(&shape),
        };
        // `cat` echoes the rough text unchanged, so the assertion does not depend
        // on rustfmt being installed.
        let out = render_file(&file, &RustRules, &Formatter::new("cat", vec![])).text;
        assert!(out.contains("use crate::crm::Customer;"));
        assert!(out.contains("pub struct Charge {"));
        assert!(out.contains("    pub amount_cents: i64,"));
        assert!(out.contains("    #[serde(default, skip_serializing_if = \"Option::is_none\")]"));
        assert!(out.contains("    pub customer: Option<Customer>,"));
    }
}

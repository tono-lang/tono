//! Assembling a whole Rust module from an IR module: the branded well-known
//! newtypes and the serde `with` helper modules once, then each shape's type
//! declaration (a struct, or the verbatim open-enum / tagged-union item). Imports
//! are derived by the engine at render time from the symbols the declarations
//! reference.

use crate::codegen::casing::CasingConfig;
use crate::codegen::targets::rust::codecs::{runtime_helpers, well_known_decls};
use crate::codegen::targets::rust::types::emit_type;
use crate::codegen::tree::File;
use crate::ir::Module;

/// Assemble a complete Rust module file for an IR module.
pub fn emit_module(module: &Module, config: &CasingConfig) -> File {
    let mut decls = well_known_decls();
    decls.extend(runtime_helpers());
    for shape in &module.shapes {
        decls.extend(emit_type(shape, config));
    }
    File {
        module: module.name.clone(),
        decls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::render::render_file;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::{enum_shape, member, structure, union_shape};
    use crate::codegen::Formatter;
    use crate::ir::{Prim, Tref};

    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    #[test]
    fn emit_module_prepends_well_known_and_helper_modules() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![structure(
                "billing#Charge",
                vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
        };
        let out = render_file(
            &emit_module(&module, &rust_casing()),
            &RustRules,
            &passthrough(),
        )
        .text;
        // Branded newtype, both integer helper modules, and the base64 module are
        // emitted once, ahead of the shape's struct.
        assert!(out.contains("#[serde(transparent)]"));
        assert!(out.contains("pub struct Timestamp(pub String);"));
        assert!(out.contains("pub mod i64_string {"));
        assert!(out.contains("pub mod u64_string {"));
        assert!(out.contains("pub mod base64_bytes {"));
        // The shape's struct routes its 64-bit field through the string codec.
        assert!(out.contains("pub struct Charge {"));
        assert!(out.contains("#[serde(with = \"i64_string\")]"));
        assert!(out.contains("pub amount_cents: i64,"));
    }

    #[test]
    fn emit_module_carries_every_shape_kind_and_collects_imports() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![
                structure(
                    "billing#Charge",
                    vec![member(
                        "customer",
                        Tref::Ref {
                            id: "crm#Customer".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                enum_shape("billing#Status", vec![("pending".into(), None)]),
                union_shape(
                    "billing#Method",
                    "type",
                    vec![member(
                        "card",
                        Tref::Ref {
                            id: "billing#card_data".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
            ],
            operations: vec![],
        };
        let out = render_file(
            &emit_module(&module, &rust_casing()),
            &RustRules,
            &passthrough(),
        )
        .text;
        // Cross-module payloads pull their import; every shape kind is present.
        assert!(out.contains("use crate::crm::Customer;"));
        assert!(out.contains("pub struct Charge {"));
        assert!(out.contains("pub enum Status {"));
        assert!(out.contains("#[serde(tag = \"type\")]"));
        assert!(out.contains("Card(CardData),"));
    }
}

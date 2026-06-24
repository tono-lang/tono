//! Assembling a whole TypeScript file from an IR module: the branded well-known
//! type aliases and the shared codec runtime helpers once, then each shape's
//! type declaration plus its codecs. Imports are derived by the engine at render
//! time from the symbols the declarations reference.

use crate::codegen::casing::CasingConfig;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::codecs::{emit_codecs, runtime_helpers};
use crate::codegen::targets::typescript::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File};
use crate::ir::Module;

/// The branded well-known type aliases: zero-dependency nominal types that are a
/// `string` underneath, distinguished only at the type level.
pub fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration"]
        .iter()
        .map(|name| {
            Decl::Alias(Alias {
                name: Symbol::builtin(*name),
                value: format!("string & {{ readonly __brand: \"{name}\" }}"),
            })
        })
        .collect()
}

/// Assemble a complete TypeScript file for an IR module.
pub fn emit_module(module: &Module, config: &CasingConfig) -> File {
    let mut decls = well_known_decls();
    decls.extend(runtime_helpers());
    for shape in &module.shapes {
        decls.extend(emit_type(shape, config));
        decls.extend(emit_codecs(shape, config));
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
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::Formatter;
    use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    #[test]
    fn well_known_aliases_are_branded_strings() {
        let out: String = well_known_decls()
            .iter()
            .map(|d| TsRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            out.contains("export type Timestamp = string & { readonly __brand: \"Timestamp\" };")
        );
        // uuid is not a branded type: it never appears among the aliases.
        assert!(!out.contains("Uuid"), "uuid is no longer branded");
    }

    #[test]
    fn emit_module_assembles_aliases_helpers_types_and_codecs() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![Shape {
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
            }],
            operations: vec![],
        };
        let out = render_file(
            &emit_module(&module, &ts_casing()),
            &TsRules,
            &passthrough(),
        )
        .text;
        // Branded alias, runtime helper, type, and codec all present and ordered.
        assert!(out.contains("export type Timestamp = string"));
        assert!(out.contains("export function encodeI64(v: bigint): string {"));
        assert!(out.contains("export interface Charge {"));
        assert!(out.contains("  amountCents: bigint;"));
        assert!(out.contains("export function encodeCharge(value: Charge): unknown {"));
        assert!(out.contains("amount_cents: encodeI64(value.amountCents),"));
    }
}

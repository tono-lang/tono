//! Assembling a TypeScript module from an IR module into separate output files so
//! the types read without the serialization noise: a types file (the branded
//! well-known aliases, each interface, each open-enum literal union, and each
//! discriminated union) and, when there is anything to serialize, a serde file
//! (the shared codec runtime helpers and each shape's `encode`/`decode`). The two
//! are separate TypeScript modules, so the serde file imports the types it
//! references from the types file (`imports_companion`); the helpers depend only on
//! built-ins, so a module of plain JSON-native types still gets only a types file.

use crate::codegen::casing::CasingConfig;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::codecs::{emit_codecs, runtime_helpers};
use crate::codegen::targets::typescript::errors;
use crate::codegen::targets::typescript::types::emit_type;
use crate::codegen::tree::{Alias, Decl, File, ModuleFile};
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

/// Assemble a TypeScript module into separate output files: a types file (the
/// branded well-known aliases and each shape's type declaration) and, when there is
/// anything to serialize, a serde file (the runtime helpers and each shape's
/// codecs). The serde file is a separate module, so it imports the module's types
/// from the types file; the runtime helpers depend only on built-ins. A module of
/// plain JSON-native types still always has codecs, so both files are emitted in
/// practice, but the serde file is omitted when no codec is produced.
pub fn emit_module(module: &Module, config: &CasingConfig) -> Vec<ModuleFile> {
    let mut type_decls = well_known_decls();
    let mut serde_decls = runtime_helpers();
    for shape in &module.shapes {
        type_decls.extend(emit_type(shape, config));
        serde_decls.extend(emit_codecs(shape, config, &module.name));
    }
    // Operations bring the error classes and the client interface into the
    // types file and the discriminators in with the codecs they call.
    if !module.operations.is_empty() {
        type_decls.extend(errors::type_decls(module, config));
        serde_decls.extend(errors::serde_decls(module));
    }

    let mut files = vec![ModuleFile {
        suffix: "",
        file: File {
            module: module.name.clone(),
            decls: type_decls,
        },
        imports_companion: None,
    }];
    // The runtime helpers are always present, so the serde file is non-empty
    // whenever the module has any shape; an empty module emits only its types.
    if serde_decls.len() > runtime_helpers().len() {
        files.push(ModuleFile {
            suffix: "_serde",
            // The serde file is the same logical module as the types file; the
            // companion redirect (below) turns the self-module type references its
            // codecs declare into an import of the types file.
            file: File {
                module: module.name.clone(),
                decls: serde_decls,
            },
            imports_companion: Some(module.name.clone()),
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::Formatter;
    use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

    fn passthrough() -> Formatter {
        Formatter::new("cat", vec![])
    }

    /// Render the text of the file with the given basename suffix ("" types,
    /// "_serde" serialization), redirecting self-module symbols to the types file.
    fn rendered(files: &[ModuleFile], suffix: &str) -> String {
        let mf = files
            .iter()
            .find(|f| f.suffix == suffix)
            .unwrap_or_else(|| panic!("module did not emit a {suffix:?} file"));
        crate::codegen::render::render_file_with_companion(
            &mf.file,
            mf.imports_companion.as_deref(),
            &TsRules,
            &passthrough(),
        )
        .text
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
        let files = emit_module(&module, &ts_casing());
        assert_eq!(files.len(), 2, "TypeScript splits types from serde");

        // The types file holds the branded aliases and the interface, with no codec
        // and no runtime helper.
        let types = rendered(&files, "");
        assert!(types.contains("export type Timestamp = string"));
        assert!(types.contains("export interface Charge {"));
        assert!(types.contains("  amountCents: bigint;"));
        assert!(!types.contains("export function encodeI64"));
        assert!(!types.contains("export function encodeCharge"));
        assert!(!types.contains("import "));

        // The serde file holds the runtime helpers and the codecs, and imports the
        // types it references from the types file.
        let serde = rendered(&files, "_serde");
        assert!(serde.contains("import { Charge } from \"./billing\";"));
        assert!(serde.contains("export function encodeI64(v: bigint): string {"));
        assert!(serde.contains("export function encodeCharge(value: Charge): unknown {"));
        assert!(serde.contains("amount_cents: encodeI64(value.amountCents),"));
        assert!(!serde.contains("export interface Charge"));
    }

    #[test]
    fn an_empty_module_emits_only_a_types_file() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![],
            operations: vec![],
        };
        let files = emit_module(&module, &ts_casing());
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].suffix, "");
    }
}

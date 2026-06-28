//! Assembling a Rust module from an IR module into separate output files so the
//! types read without the serialization noise: a types file (the branded
//! well-known newtypes, each struct, each tagged-union enum, and each open enum's
//! bare definition) and, when there is custom serde to emit, a serde file (the
//! `#[serde(with)]` helper modules and each open enum's hand-written impls). The
//! two are separate Rust modules of one crate, so the cross-file `use`s the split
//! creates ride at the top of each file: the types file pulls the helper modules
//! it routes fields through (`use crate::<module>_serde::{...}`), and the serde
//! file pulls the module's types so its enum impls resolve (`use crate::<module>::*`).
//! Imports of cross-*module* types are still derived by the engine.

use crate::codegen::casing::CasingConfig;
use crate::codegen::targets::rust::codecs::{runtime_helpers, well_known_decls, HelperSet};
use crate::codegen::targets::rust::types::{emit_serde, emit_type};
use crate::codegen::tree::{Decl, File, ModuleFile, Raw};
use crate::ir::Module;

/// Assemble a Rust module into separate output files: a types file (well-known
/// newtypes and each shape's type declaration) and, when there is custom serde, a
/// serde file (the `#[serde(with)]` helper modules and the open enums' impls). The
/// types file routes its wide-integer and bytes fields through helper modules that
/// live in the serde file, so it `use`s exactly those; the serde file's enum impls
/// reference the module's types, so it `use`s the whole types module. A module of
/// plain structs with no wide integer, no bytes, and no open enum needs no serde
/// file at all.
pub fn emit_module(module: &Module, config: &CasingConfig) -> Vec<ModuleFile> {
    let mut type_decls = well_known_decls();
    let mut serde_shape_decls = Vec::new();
    let mut helpers = HelperSet::default();
    for shape in &module.shapes {
        let types = emit_type(shape, config);
        for decl in &types {
            if let Decl::Interface(interface) = decl {
                for field in &interface.fields {
                    helpers.add_field(field);
                }
            }
        }
        type_decls.extend(types);
        serde_shape_decls.extend(emit_serde(shape));
    }

    // The serde file holds the used helper modules and the open enums' impls. When
    // both are empty there is nothing to serialize beyond serde's derives.
    let has_serde = !helpers.is_empty() || !serde_shape_decls.is_empty();
    if has_serde {
        // The types file routes fields through the helper modules, so it imports
        // exactly the ones it uses from the serde file.
        if let Some(import) = helpers_use(module, helpers) {
            type_decls.insert(0, import);
        }
    }

    let mut files = vec![ModuleFile {
        suffix: "",
        file: File {
            module: module.name.clone(),
            decls: type_decls,
        },
        imports_companion: None,
    }];

    if has_serde {
        let mut serde_decls = runtime_helpers(helpers);
        // The enum impls reference the module's types, so the serde file pulls them
        // in; with no enum impl there is nothing referencing the types, so the glob
        // would be unused.
        if !serde_shape_decls.is_empty() {
            serde_decls.insert(0, types_glob_use(module));
        }
        serde_decls.extend(serde_shape_decls);
        files.push(ModuleFile {
            suffix: "_serde",
            file: File {
                module: module.name.clone(),
                decls: serde_decls,
            },
            imports_companion: None,
        });
    }
    files
}

/// The types file's `use crate::<module>_serde::{<helpers>};`, or `None` when no
/// field routes through a helper module. The `with = "..."` attribute paths on the
/// struct fields resolve through this import.
fn helpers_use(module: &Module, helpers: HelperSet) -> Option<Decl> {
    let names = helpers.names();
    if names.is_empty() {
        return None;
    }
    Some(raw_use(format!(
        "use crate::{}_serde::{{{}}};",
        module.name,
        names.join(", ")
    )))
}

/// The serde file's `use crate::<module>::*;`, which brings the module's types into
/// scope so the open enums' impls (and the orphan-rule local-type requirement) resolve.
fn types_glob_use(module: &Module) -> Decl {
    raw_use(format!("use crate::{}::*;", module.name))
}

/// A verbatim `use` item carrying no symbol references (the engine must not treat
/// the path as an importable symbol).
fn raw_use(text: String) -> Decl {
    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
    })
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

    /// Render the text of the file with the given basename suffix ("" types,
    /// "_serde" serialization), panicking if the module did not emit it.
    fn rendered(module: &Module, suffix: &str) -> String {
        let files = emit_module(module, &rust_casing());
        let mf = files
            .iter()
            .find(|f| f.suffix == suffix)
            .unwrap_or_else(|| panic!("module did not emit a {suffix:?} file"));
        render_file(&mf.file, &RustRules, &passthrough()).text
    }

    #[test]
    fn a_module_with_a_wide_integer_field_splits_helpers_into_the_serde_file() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![structure(
                "billing#Charge",
                vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
        };
        assert_eq!(emit_module(&module, &rust_casing()).len(), 2);

        // The types file holds the branded newtype and the struct, and pulls only the
        // i64 helper it uses from the serde file (no u64, no base64).
        let types = rendered(&module, "");
        assert!(types.contains("#[serde(transparent)]"));
        assert!(types.contains("pub struct Timestamp(pub String);"));
        assert!(types.contains("use crate::billing_serde::{i64_string};"));
        assert!(!types.contains("u64_string"));
        assert!(!types.contains("base64_bytes"));
        assert!(types.contains("pub struct Charge {"));
        assert!(types.contains("#[serde(with = \"i64_string\")]"));
        assert!(types.contains("pub amount_cents: i64,"));
        // The helper module's body and any glob import stay out of the types file.
        assert!(!types.contains("pub mod i64_string {"));
        assert!(!types.contains("use crate::billing::*;"));

        // The serde file holds exactly the i64 helper module; with no open enum it
        // needs no glob import of the types.
        let serde = rendered(&module, "_serde");
        assert!(serde.contains("pub mod i64_string {"));
        assert!(serde.contains("if s.is_human_readable() {"));
        assert!(!serde.contains("pub mod u64_string {"));
        assert!(!serde.contains("pub mod base64_bytes {"));
        assert!(!serde.contains("use crate::billing::*;"));
    }

    #[test]
    fn a_plain_struct_module_emits_only_a_types_file() {
        let module = Module {
            name: "billing".into(),
            shapes: vec![structure(
                "billing#Note",
                vec![member("text", Tref::Prim(Prim::String), true)],
            )],
            operations: vec![],
        };
        let files = emit_module(&module, &rust_casing());
        // No wide integer, no bytes, no open enum: nothing to serialize beyond derives.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].suffix, "");
        let types = rendered(&module, "");
        assert!(types.contains("pub struct Note {"));
        assert!(!types.contains("use crate::billing_serde"));
    }

    #[test]
    fn an_open_enum_splits_its_definition_from_its_impls() {
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
        let types = rendered(&module, "");
        // Cross-module payloads pull their import; the open enum's definition and the
        // tagged union both live in the types file; the enum's impls do not.
        assert!(types.contains("use crate::crm::Customer;"));
        assert!(types.contains("pub struct Charge {"));
        assert!(types.contains("pub enum Status {"));
        assert!(types.contains("Unknown(String),"));
        assert!(types.contains("#[serde(tag = \"type\")]"));
        assert!(types.contains("Card(CardData),"));
        assert!(!types.contains("impl serde::Serialize for Status"));
        // No helper module is used here, so the types file imports nothing from serde.
        assert!(!types.contains("use crate::billing_serde"));

        // The serde file pulls the module's types in and holds the enum impls.
        let serde = rendered(&module, "_serde");
        assert!(serde.contains("use crate::billing::*;"));
        assert!(serde.contains("impl Status {"));
        assert!(serde.contains("impl serde::Serialize for Status"));
        assert!(serde.contains("impl<'de> serde::Deserialize<'de> for Status"));
        // The struct, union, and enum definitions stay out of the serde file.
        assert!(!serde.contains("pub struct Charge"));
        assert!(!serde.contains("pub enum Status {"));
    }
}

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
//!
//! A declared entry gets a group of its own per declaration (see
//! [`crate::codegen::targets::rust::entry`]), named after it, since its
//! construction surface references only itself, the module's shared config
//! structs, and the SDK-root resolution helpers — never the types/serde split
//! above.

use crate::codegen::casing::CasingConfig;
use crate::codegen::group::Group;
use crate::codegen::targets::rust::codecs::{open_enum_macro, runtime_helpers, HelperSet};
use crate::codegen::targets::rust::errors;
use crate::codegen::targets::rust::types::{emit_serde, emit_type, emit_validators};
use crate::codegen::taxonomy;
use crate::codegen::tree::{Decl, ModuleFile, Raw};
use crate::codegen::validation;
use crate::codegen::visibility::Exposed;
use crate::ir::Module;

/// The binding-language key the Rust target reads, as [`taxonomy::derive`]
/// and [`crate::codegen::extensions::bound_extensions`] take it.
const BINDING_LANGS: [&str; 1] = ["rust"];

/// Assemble a Rust module into separate output files: a types file (well-known
/// newtypes and each shape's type declaration) and, when there is custom serde, a
/// serde file (the `#[serde(with)]` helper modules and the open enums' impls). The
/// types file routes its wide-integer and bytes fields through helper modules that
/// live in the serde file, so it `use`s exactly those; the serde file's enum impls
/// reference the module's types, so it `use`s the whole types module. A module of
/// plain structs with no wide integer, no bytes, and no open enum needs no serde
/// file at all.
pub fn emit_module(module: &Module, config: &CasingConfig, exposed: &Exposed) -> Vec<ModuleFile> {
    let mut type_decls = Vec::new();
    let mut internal_type_decls = Vec::new();
    let mut codec_shape_decls = Vec::new();
    let mut hidden_serde_decls = Vec::new();
    // The helper modules are imported per file, since a `#[serde(with = "...")]`
    // path resolves in the scope of the struct carrying it; a group that routes
    // no field through a helper must not import one.
    let mut public_helpers = HelperSet::default();
    let mut internal_helpers = HelperSet::default();
    // Whether anything in the internal group names a type from the public one.
    let mut serde_needs_types = false;
    for shape in &module.shapes {
        let types = emit_type(shape, config);
        // A shape a public type reaches is public; the rest are the module's own
        // business and stay in its internal group.
        let public = exposed.shape(shape);
        let helpers = if public {
            &mut public_helpers
        } else {
            &mut internal_helpers
        };
        for decl in &types {
            if let Decl::Interface(interface) = decl {
                for field in &interface.fields {
                    helpers.add_field(field);
                }
            }
        }
        let into = if public {
            &mut type_decls
        } else {
            &mut internal_type_decls
        };
        into.extend(types);
        // Validators live with the type they check.
        into.extend(emit_validators(shape, config));
        // A hidden shape's impls go with it; a public one's stay in the codec
        // group, which sits beside the types (an impl has to be where its type
        // is).
        let serde = emit_serde(shape);
        if public {
            serde_needs_types |= !serde.is_empty();
            codec_shape_decls.extend(serde);
        } else {
            hidden_serde_decls.extend(serde);
        }
    }

    // Operations bring the taxonomy (with the Api payload enum) and the client
    // trait into the public group, and the error discriminators with them: Rust
    // has no generated client, so the trait is the consumer's to implement and
    // turning a status and a body into the taxonomy is part of the surface they
    // implement it against, not serialization the SDK keeps to itself.
    if !module.operations.is_empty() {
        type_decls.extend(errors::type_decls(module, config));
        type_decls.extend(errors::serde_decls(module));
        // Bespoke boundary wrappers sit in the types file next to the error
        // taxonomy they map to, so they ride the operation surface: a module with
        // no operations has no ContractError to wrap a failure into. A pure-contract
        // module still trips the conformance gate, it just emits no wrapper until an
        // operation gives it the error surface (or the concrete client lands).
        type_decls.extend(crate::codegen::targets::rust::client::wrapper_decls(module));
    } else if crate::codegen::entries::has_entries(module) {
        // A module whose operations live in an entry keeps the error taxonomy
        // (the wire error shapes reference it, and downstream consumers match
        // on it) plus the boundary wrappers any bound contract/constraint
        // needs (mirrors the loose-op branch above).
        type_decls.extend(errors::taxonomy_and_declared_decls(
            module,
            &taxonomy::derive_rust_entry(module, &BINDING_LANGS),
        ));
        type_decls.extend(crate::codegen::targets::rust::client::wrapper_decls(module));
    } else if module.shapes.iter().any(validation::shape_has_checks) {
        // Constraints without operations still need the Validation category a
        // validator returns (`Violation`, `ValidationError`), which the taxonomy
        // would otherwise have carried.
        type_decls.extend(errors::standalone_validation_decls());
    }

    let mut codec_decls = Vec::new();
    let mut internal_decls = Vec::new();
    // The helper modules live in the SDK's shared internal module, and the
    // `with = "..."` attribute paths resolve through this import.
    // One `use` per shared group the fields reach, since a helper's group is
    // named for what it holds and a module may route through more than one.
    for (group, names) in public_helpers.by_group().into_iter().rev() {
        type_decls.insert(0, helpers_use(&Group::root(group), &names));
    }
    for (group, names) in internal_helpers.by_group().into_iter().rev() {
        internal_type_decls.insert(0, helpers_use(&Group::root(group), &names));
    }

    // The open enums' impls reference the types they are for, so the codec group
    // pulls the public ones in; without one, nothing references them and the
    // glob would be unused. The `open_enum!` macro is defined once per group,
    // before the invocations that expand it, so each open enum is one invocation
    // rather than a repeated impl block.
    if serde_needs_types {
        codec_decls.push(types_glob_use(&Group::types(&module.name)));
    }
    if !codec_shape_decls.is_empty() {
        codec_decls.push(open_enum_macro());
    }
    codec_decls.extend(codec_shape_decls);
    if !hidden_serde_decls.is_empty() {
        internal_decls.push(open_enum_macro());
    }
    internal_decls.extend(hidden_serde_decls);
    internal_decls.extend(internal_type_decls);

    // A module's entries own their own construction surface (Settings,
    // builder/constructor, Client, op methods, discriminators), independent
    // of whether the module also has loose operations (today a module never
    // has both). The construction-only config structs `@bind` composes into
    // may serve more than one entry, so they ride the module's public group
    // alongside its other shared types; each entry declaration otherwise gets
    // its own group, named after it (mirrors Go's `targets/go/emit.rs`), so
    // the construction surface reads together instead of being split across a
    // types and a codec file.
    let entries = crate::codegen::targets::rust::entry::emit(module, config);
    type_decls.extend(entries.shared);

    let mut files = vec![ModuleFile::new(Group::types(&module.name), type_decls)];
    if !codec_decls.is_empty() {
        files.push(ModuleFile::new(Group::codec(&module.name), codec_decls));
    }
    if !internal_decls.is_empty() {
        files.push(ModuleFile::new(
            Group::module_internal(&module.name),
            internal_decls,
        ));
    }
    for (name, decls) in entries.per_entry {
        files.push(ModuleFile::new(Group::entry(&module.name, &name), decls));
    }
    files.extend(crate::codegen::targets::rust::entry::vector_tests::test_files(module, config));
    files
}

/// Whether any module in the model declares an entry, which is what puts
/// anything in the entry resolution's shared groups.
fn model_has_entries(model: &crate::ir::Model) -> bool {
    model
        .modules
        .iter()
        .any(crate::codegen::entries::has_entries)
}

/// The SDK-root group's declarations: the `#[serde(with)]` helper modules any
/// field in the model routes through, plus (when the model declares any entry)
/// the resolution helpers every entry's construction logic may call — reading
/// an environment variable, parsing a duration, and the `@str::` casing
/// transforms. They serve no module in particular, so the whole crate carries
/// one copy instead of one per module, and the assembler prunes what nothing
/// in the emitted SDK references.
pub fn shared_groups(
    model: &crate::ir::Model,
    config: &CasingConfig,
) -> Vec<(&'static str, Vec<Decl>)> {
    let helpers = shared_helpers(model, config);
    let mut groups: Vec<(&'static str, Vec<Decl>)> = helpers
        .by_group()
        .into_iter()
        .map(|(group, _)| (group, runtime_helpers(helpers, group)))
        .collect();
    if model_has_entries(model) {
        groups.extend(crate::codegen::targets::rust::entry::shared_groups());
    }
    groups
}

/// Which `#[serde(with)]` helper modules the whole model reaches. An entry's
/// own bytes-typed field also reaches `base64_bytes` when it declares `@env`
/// as a source (the only entry construction path that calls it, mirroring a
/// wire struct field's own env-sourced boundary); this set gates whether the
/// `base64_bytes` module is emitted at all, so it has to be exact, unlike the
/// `env`/`duration`/`casing` groups whose declarations the assembler prunes
/// after the fact.
fn shared_helpers(model: &crate::ir::Model, config: &CasingConfig) -> HelperSet {
    let mut helpers = HelperSet::default();
    for module in &model.modules {
        for shape in &module.shapes {
            for decl in emit_type(shape, config) {
                if let Decl::Interface(interface) = decl {
                    for field in &interface.fields {
                        helpers.add_field(field);
                    }
                }
            }
        }
        for entry in crate::codegen::entries::module_entries(module) {
            for field in &entry.fields {
                if matches!(field.target, crate::ir::Tref::Prim(crate::ir::Prim::Bytes))
                    && field
                        .sources
                        .iter()
                        .any(|s| matches!(s, crate::ir::Source::Env(_)))
                {
                    helpers.base64_bytes.plain = true;
                }
            }
        }
    }
    helpers
}

/// The `use` that brings the named declarations of an SDK-root group into
/// scope: the `#[serde(with)]` helper modules a struct field routes through,
/// or the resolution helpers an entry's construction logic calls.
pub(crate) fn helpers_use(from: &Group, names: &[&str]) -> Decl {
    raw_use(format!(
        "use {}::{{{}}};",
        crate::codegen::layout::rust_path(&from.path()).unwrap_or_default(),
        names.join(", ")
    ))
}

/// The glob `use` that brings a group's types into scope: the open enums'
/// impls (and the orphan-rule local-type requirement), or an entry's own
/// group reaching the module's error taxonomy and declared-error types
/// without threading a `Symbol` ref through every generated call site that
/// names one.
pub(crate) fn types_glob_use(types: &Group) -> Decl {
    raw_use(format!(
        "use {}::*;",
        crate::codegen::layout::rust_path(&types.path()).unwrap_or_default()
    ))
}

/// A verbatim `use` item carrying no symbol references (the engine must not treat
/// the path as an importable symbol).
fn raw_use(text: String) -> Decl {
    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
        ..Raw::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::group::{CODEC, TYPES};
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::{
        constrained_module, enum_shape, member, structure, union_shape, wire_binding,
    };
    use crate::ir::{Prim, Tref};

    #[test]
    fn constraints_without_operations_still_carry_the_validation_category() {
        // No operation means no taxonomy, but the validator returns the Validation
        // category, so its types must still be emitted or the module cannot compile.
        let types = rendered(&constrained_module(), TYPES);
        assert!(types.contains("pub struct Violation {"));
        assert!(types.contains("pub struct ValidationError {"));
        assert!(types.contains("pub violations: Vec<Violation>,"));
        assert!(types.contains("pub fn validate(&self) -> Result<(), ValidationError> {"));
        // Only the category the validator needs, not the rest of the taxonomy.
        assert!(!types.contains("pub enum TonoError"));
    }

    /// Emit a module's groups with everything exposed, resolved the way the
    /// pipeline resolves them.
    fn groups(module: &Module) -> Vec<ModuleFile> {
        crate::codegen::test_support::resolve_groups(
            emit_module(module, &rust_casing(), &Exposed::all()),
            crate::codegen::TargetKind::Rust,
        )
    }

    /// Render the named group of a module, panicking if it did not emit one.
    fn rendered(module: &Module, group: &str) -> String {
        crate::codegen::test_support::render_group(
            &groups(module),
            group,
            crate::codegen::TargetKind::Rust,
            &RustRules::default(),
        )
    }

    #[test]
    fn a_module_whose_operations_live_in_an_entry_keeps_the_error_taxonomy() {
        // An entry always goes through a fallible construction, so `Config`
        // stays live even with no operations; the loose-op client trait,
        // which has no operation here to interface, stays out.
        let module = Module {
            tests: vec![],
            name: "notes".into(),
            shapes: vec![crate::ir::Shape {
                id: "notes#client".into(),
                kind: crate::ir::ShapeKind::Entry {
                    fields: vec![],
                    operations: vec![],
                },
                traits: vec![],
            }],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        };
        let types = rendered(&module, TYPES);
        assert!(types.contains("pub enum TonoError"));
        assert!(types.contains("pub struct ConfigError"));
        assert!(!types.contains("pub trait Client"));
        // Nothing in an entry with no operations ever calls the runtime, so
        // there is no transport call site to fail, and no bespoke boundary
        // exists to wrap a foreign error into a contract failure either.
        assert!(!types.contains("pub struct TransportError"));
        assert!(!types.contains("pub struct ContractError"));
    }

    #[test]
    fn an_entry_with_a_wire_operation_keeps_transport_decode_and_api() {
        let mut wire = wire_binding("GET");
        wire.endpoint = Some(crate::ir::WireValue::Field(vec!["ep".into()]));
        let op = crate::ir::Shape {
            id: "notes#client.ping".into(),
            kind: crate::ir::ShapeKind::Operation {
                input: None,
                input_name: None,
                output: None,
                output_nullable: false,
                errors: vec![],
                wire: Some(wire),
                impl_call: None,
            },
            traits: vec![],
        };
        let ep = crate::ir::EntryField {
            name: "ep".into(),
            target: Tref::Prim(Prim::String),
            sources: vec![
                crate::ir::Source::With,
                crate::ir::Source::Default(serde_json::json!("https://example.com")),
            ],
            format: None,
            transforms: vec![],
            select: None,
            call: None,
            handle_call: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        };
        let module = Module {
            tests: vec![],
            name: "notes".into(),
            shapes: vec![crate::ir::Shape {
                id: "notes#client".into(),
                kind: crate::ir::ShapeKind::Entry {
                    fields: vec![ep],
                    operations: vec![op],
                },
                traits: vec![],
            }],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        };
        let types = rendered(&module, TYPES);
        assert!(types.contains("pub struct TransportError"));
        assert!(types.contains("pub struct DecodeError"));
        assert!(types.contains("pub struct APIError"));
        assert!(types.contains("pub enum APIFailure"));
        // No extension is bound, but the entry runtime call's catch-all match
        // arm (`entry/mod.rs::op_method`) wraps any downcast failure into
        // ContractError as unconditional generated text for every wire-op
        // entry method, regardless of whether this module binds one (see
        // `taxonomy::derive_rust_entry`).
        assert!(types.contains("pub struct ContractError"));
        assert!(!types.contains("pub struct Violation"));
        assert!(!types.contains("pub struct ValidationError"));
    }

    #[test]
    fn a_module_with_a_wide_integer_field_splits_helpers_into_the_serde_file() {
        let module = Module {
            tests: vec![],
            name: "billing".into(),
            shapes: vec![structure(
                "billing#Charge",
                vec![
                    member("amount_cents", Tref::Prim(Prim::I64), true),
                    member("created", Tref::Prim(Prim::Timestamp), true),
                ],
            )],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        };
        assert_eq!(groups(&module).len(), 1);

        // The types group holds the struct and pulls only the i64 helper it uses
        // from the shared module (no u64, no base64); with one module the
        // branded newtype the field names folds in here too, and the ones nothing
        // names are left out.
        let types = rendered(&module, TYPES);
        assert!(types.contains("pub struct Timestamp(pub String);"));
        assert!(!types.contains("pub struct LocalDate"));
        assert!(!types.contains("pub struct Duration"));
        assert!(types.contains("use crate::number::{i64_string};"));
        assert!(!types.contains("u64_string"));
        assert!(!types.contains("base64_bytes"));
        assert!(types.contains("pub struct Charge {"));
        assert!(types.contains("#[serde(with = \"i64_string\")]"));
        assert!(types.contains("pub amount_cents: i64,"));
        // The helper module's body and any glob import stay out of the types file.
        assert!(!types.contains("pub mod i64_string {"));
        assert!(!types.contains("use crate::billing::types::*;"));

        // The helper module itself is shared across the SDK, so it is not the
        // module's to emit, and it lands in the group named for what it holds.
        let groups = shared_groups(
            &crate::ir::Model {
                tono_ir_version: 6,
                modules: vec![module.clone()],
            },
            &rust_casing(),
        );
        let names: Vec<&str> = groups.iter().map(|(name, _)| *name).collect();
        assert_eq!(names, vec!["number"], "no bytes field, so no bytes group");
        let shared = crate::codegen::test_support::render_group(
            &[ModuleFile::new(
                Group::root(groups[0].0),
                groups[0].1.clone(),
            )],
            groups[0].0,
            crate::codegen::TargetKind::Rust,
            &RustRules::default(),
        );
        assert!(shared.contains("pub mod i64_string {"));
        assert!(shared.contains("if s.is_human_readable() {"));
        assert!(!shared.contains("pub mod u64_string {"));
    }

    #[test]
    fn a_plain_struct_module_emits_only_a_types_file() {
        let module = Module {
            tests: vec![],
            name: "billing".into(),
            shapes: vec![structure(
                "billing#Note",
                vec![member("text", Tref::Prim(Prim::String), true)],
            )],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        };
        let files = groups(&module);
        // No wide integer, no bytes, no open enum: nothing to serialize beyond derives.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].group.name, TYPES);
        let types = rendered(&module, TYPES);
        assert!(types.contains("pub struct Note {"));
        assert!(!types.contains("use crate::internal"));
    }

    #[test]
    fn an_open_enum_splits_its_definition_from_its_impls() {
        let module = Module {
            tests: vec![],
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
            extensions: vec![],
            ext_libs: vec![],
        };
        let types = rendered(&module, TYPES);
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
        assert!(!types.contains("use crate::internal"));

        // The serde file pulls the module's types in, defines the shared codec macro
        // once, and expands it per enum through an invocation.
        let serde = rendered(&module, CODEC);
        assert!(serde.contains("use crate::billing::types::*;"));
        assert!(serde.contains("macro_rules! open_enum {"));
        assert!(serde.contains("impl serde::Serialize for $name"));
        assert!(serde.contains("open_enum!(Status: String {"));
        // The macro is defined a single time even with the enum present.
        assert_eq!(serde.matches("macro_rules! open_enum {").count(), 1);
        // The struct, union, and enum definitions stay out of the serde file.
        assert!(!serde.contains("pub struct Charge"));
        assert!(!serde.contains("pub enum Status {"));
    }
}

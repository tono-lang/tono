//! Assembling a Go module from an IR module into separate output files so the types
//! can be read without the serialization noise: a types file (the branded well-known
//! named strings, the `Entry`/`Entries` definitions when `@entries` is used, and
//! each shape's type declarations — structs, enums, and a union's interface,
//! wrappers, and markers) and, when there is anything to serialize, a serde file
//! (`marshalVariant`, the `Entries` (de)serialization methods, each union's wrapper
//! `MarshalJSON`s and `UnmarshalX`, and each container's `UnmarshalJSON`). Imports
//! are derived per file from the symbols its declarations reference, so the types
//! file pulls nothing while the serde file pulls `encoding/json` (plus `fmt` for a
//! union); a module of plain tagged structs emits only the types file.
//!
//! The Go package clause is not part of a rendered file (the engine emits imports
//! first); the caller prepends `package <name>` before formatting, once per file.
//! See [`package_clause`].

use std::collections::HashSet;

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::has_entries;
use crate::codegen::group::Group;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::codecs::{
    emit_serde_decls, runtime_serde_helpers, runtime_type_helpers, RuntimeHelpers,
};
use crate::codegen::targets::go::errors;
use crate::codegen::targets::go::types::{emit_type, emit_validators};
use crate::codegen::tree::{Alias, Decl, ModuleFile};
use crate::codegen::validation;
use crate::codegen::visibility::Exposed;
use crate::ir::{Module, ShapeKind};

/// The branded well-known types: distinct named string types, so they serialize
/// exactly as their inner value while staying distinct in code.
pub fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration"]
        .iter()
        .map(|name| {
            Decl::Alias(Alias {
                name: Symbol::builtin(*name),
                value: "string".into(),
            })
        })
        .collect()
}

/// The Go package clause for a module name, which the caller prepends before
/// formatting (the rendered file starts with imports, so the clause cannot be a
/// declaration).
pub fn package_clause(name: &str) -> String {
    format!("package {name}\n")
}

/// The SDK-root group's declarations: the construction helpers every entry of
/// every module calls. They serve no module in particular, so they sit in the
/// SDK's shared `internal/` package rather than being repeated per module.
///
/// Nothing type-level goes here. A method has to live in its receiver's package,
/// and a type a public struct exposes has to be nameable by a consumer, which
/// `internal/` forbids; both keep the module's own package.
pub fn shared_decls(model: &crate::ir::Model) -> Vec<Decl> {
    if !model
        .modules
        .iter()
        .any(crate::codegen::entries::has_entries)
    {
        return Vec::new();
    }
    crate::codegen::targets::go::entry::runtime_decls()
}

/// Whether any structure member in the module carries the `@entries` escape, which
/// is the only thing that pulls the generic `Entries[K, V]` helper.
fn uses_entries(module: &Module) -> bool {
    module.shapes.iter().any(|s| match &s.kind {
        ShapeKind::Structure { members, .. } => members.iter().any(|m| has_entries(&m.traits)),
        _ => false,
    })
}

/// Whether the module has any union, which pulls the `marshalVariant` helper.
fn uses_union(module: &Module) -> bool {
    module
        .shapes
        .iter()
        .any(|s| matches!(s.kind, ShapeKind::Union { .. }))
}

/// Assemble a Go module into separate output files: the types file (well-known
/// named strings, the `Entry`/`Entries` definitions when `@entries` is used, and
/// each shape's type declarations) and, when there is any serialization to emit,
/// the serde file (`marshalVariant`, the `Entries` (de)serialization methods, each
/// union's wrapper `MarshalJSON`s and `UnmarshalX`, and each container's
/// `UnmarshalJSON`). A module of plain tagged structs emits only the types file:
/// `encoding/json` does all its work, so there is nothing for the serde file to
/// hold. Imports are derived per file from the symbols its declarations reference,
/// so the types file pulls nothing while the serde file pulls `encoding/json`
/// (plus `fmt` when a union is present).
pub fn emit_module(
    module: &Module,
    config: &CasingConfig,
    union_ids: &HashSet<String>,
    exposed: &Exposed,
) -> Vec<ModuleFile> {
    let helpers = RuntimeHelpers {
        entries: uses_entries(module),
        variant: uses_union(module),
    };
    // The branded well-known strings and the `Entries` container are part of the
    // module's public surface (a struct field has one), and a method needs its
    // receiver's package, so both stay here rather than in the shared package.
    let mut type_decls = well_known_decls();
    type_decls.extend(runtime_type_helpers(helpers));
    let mut serde_decls = runtime_serde_helpers(helpers);
    // A shape a public type reaches is public; the rest are the module's own
    // business and stay in its internal group.
    let mut internal_decls = Vec::new();
    for shape in &module.shapes {
        let into = if exposed.shape(shape) {
            &mut type_decls
        } else {
            &mut internal_decls
        };
        into.extend(emit_type(shape, config));
        // Validators live with the type they check.
        into.extend(emit_validators(shape, config));
        serde_decls.extend(emit_serde_decls(shape, config, union_ids, &module.name));
    }
    let module_has_entries = crate::codegen::entries::has_entries(module);
    // Operations bring the error values and the blocking client interface
    // into the types file; the discriminators unmarshal, so they land with
    // the serialization.
    if !module.operations.is_empty() {
        type_decls.extend(errors::type_decls(module, config));
        serde_decls.extend(errors::serde_decls(module));
        // Bespoke boundary wrappers sit next to the error values they map to, so
        // they ride the operation surface: a module with no operations has no
        // ContractError to wrap a failure into. A pure-contract module still trips
        // the conformance gate, it just emits no wrapper until an operation gives it
        // the error surface (or the concrete client lands).
        type_decls.extend(crate::codegen::targets::go::client::wrapper_decls(module));
    } else if module_has_entries {
        // An entry's client maps outcomes onto the same taxonomy; its client
        // surface is the entry struct, so the loose-op interface stays out.
        type_decls.extend(errors::taxonomy_and_declared_decls(module));
        type_decls.extend(crate::codegen::targets::go::client::wrapper_decls(module));
    } else if module.shapes.iter().any(validation::shape_has_checks) {
        // Constraints without operations still need the Validation category a
        // validator returns (`Violation`, `ValidationError`), which the taxonomy
        // would otherwise have carried.
        type_decls.extend(errors::standalone_validation_decls());
    }
    let entries = crate::codegen::targets::go::entry::emit(module, config);
    internal_decls.extend(entries.shared);
    internal_decls.extend(serde_decls);

    let mut files = vec![ModuleFile::new(Group::types(&module.name), type_decls)];
    // One group per entry declaration, named after it: the entry's own type, its
    // constructor, and its operation methods, so the construction surface reads
    // together instead of being split across a types and a codec file.
    for (name, decls) in entries.per_entry {
        files.push(ModuleFile::new(Group::entry(&module.name, &name), decls));
    }
    // A pure-types module (no union, no @entries, no union-bearing container,
    // nothing hidden) has nothing internal to emit.
    if !internal_decls.is_empty() {
        files.push(ModuleFile::new(
            Group::module_internal(&module.name),
            internal_decls,
        ));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::group::{INTERNAL, TYPES};
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{member, structure, union_shape};
    use crate::ir::{Prim, Tref};

    /// A single module's own union ids, the set the pipeline builds model-wide.
    fn union_ids(module: &Module) -> HashSet<String> {
        module
            .shapes
            .iter()
            .filter(|s| matches!(s.kind, ShapeKind::Union { .. }))
            .map(|s| s.id.clone())
            .collect()
    }

    /// Emit a module's groups with everything exposed, resolved the way the
    /// pipeline resolves them.
    fn groups(module: &Module) -> Vec<ModuleFile> {
        crate::codegen::test_support::resolve_groups(emit_module(
            module,
            &go_casing(),
            &union_ids(module),
            &Exposed::all(),
        ))
    }

    /// Render the named group of a module, panicking if it did not emit one.
    fn rendered(files: &[ModuleFile], group: &str) -> String {
        crate::codegen::test_support::render_group(
            files,
            group,
            crate::codegen::TargetKind::Go,
            &GoRules::default(),
        )
    }

    #[test]
    fn constraints_without_operations_still_carry_the_validation_category() {
        // No operation means no taxonomy, but the validator returns the Validation
        // category, so its types must still be emitted or the package cannot compile.
        let module = crate::codegen::test_support::constrained_module();
        let types = rendered(&groups(&module), TYPES);
        assert!(types.contains("type Violation struct {"));
        assert!(types.contains("type ValidationError struct {"));
        assert!(types.contains("func (e *ValidationError) Error() string"));
        // The category stays inside the sealed taxonomy a bound hook matches on.
        assert!(types.contains("func (e *ValidationError) sdkError() {}"));
        assert!(types.contains("func ValidateCharge(value Charge) error {"));
        // Only the category the validator needs, not the rest of the taxonomy.
        assert!(!types.contains("type TransportError struct"));
    }

    #[test]
    fn the_package_clause_names_the_module() {
        assert_eq!(package_clause("models"), "package models\n");
    }

    #[test]
    fn a_module_of_plain_structs_emits_only_a_types_file_with_no_imports() {
        let module = Module {
            name: "models".into(),
            shapes: vec![structure(
                "models#Charge",
                vec![member("amount_cents", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
            extensions: vec![],
        };
        let files = groups(&module);
        // A pure-types module emits a single file: there is no serialization to hold.
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].group.name, TYPES);
        let out = rendered(&files, TYPES);
        // Well-known named strings and the tagged type are present; with no union and
        // no @entries, no runtime helper and no import is emitted.
        assert!(out.contains("type Timestamp string"));
        assert!(out.contains("type Charge struct {"));
        // The type holds the 64-bit integer natively, tagged `,string`.
        assert!(out.contains("\tAmountCents int64 `json:\"amount_cents,string\"`\n"));
        assert!(!out.contains("func marshalVariant("));
        assert!(!out.contains("type Entries["));
        assert!(!out.contains("import "));
    }

    #[test]
    fn a_module_with_a_union_splits_types_from_serde() {
        let module = Module {
            name: "models".into(),
            shapes: vec![
                structure(
                    "models#Account",
                    vec![member(
                        "method",
                        Tref::Ref {
                            id: "models#Method".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                union_shape(
                    "models#Method",
                    "type",
                    vec![member(
                        "card",
                        Tref::Ref {
                            id: "models#card_data".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                structure(
                    "models#card_data",
                    vec![member("last4", Tref::Prim(Prim::String), true)],
                ),
            ],
            operations: vec![],
            extensions: vec![],
        };
        let files = groups(&module);
        assert_eq!(files.len(), 2);

        // The types file holds the interface, wrappers, markers, and the struct types,
        // with no serialization and no import at all.
        let types = rendered(&files, TYPES);
        assert!(types.contains("type Method interface{ isMethod() }"));
        assert!(types.contains("type MethodCard struct{ Value CardData }"));
        assert!(types.contains("func (MethodCard) isMethod() {}"));
        assert!(types.contains("type Account struct {"));
        assert!(types.contains("type CardData struct {"));
        assert!(!types.contains("import "));
        assert!(!types.contains("MarshalJSON"));
        assert!(!types.contains("UnmarshalJSON"));
        assert!(!types.contains("func marshalVariant("));

        // The serde file holds marshalVariant, the wrapper MarshalJSON, the dispatcher,
        // and the container UnmarshalJSON; it pulls encoding/json and fmt, but never
        // imports the module itself (the payload type is same-package).
        let serde = rendered(&files, INTERNAL);
        assert!(serde.contains("func marshalVariant("));
        assert!(serde.contains(
            "func (m MethodCard) MarshalJSON() ([]byte, error) { return marshalVariant("
        ));
        assert!(serde.contains("func UnmarshalMethod(b []byte) (Method, error) {"));
        assert!(serde.contains("func (a *Account) UnmarshalJSON(b []byte) error {"));
        assert!(serde.contains("import \"encoding/json\""));
        assert!(serde.contains("import \"fmt\""));
        assert!(!serde.contains("import \"models\""));
        // The interface and wrapper definitions stay out of the serde file.
        assert!(!serde.contains("type Method interface"));
        assert!(!serde.contains("type MethodCard struct"));
    }

    #[test]
    fn a_module_with_an_entries_field_splits_the_definition_from_its_methods() {
        let mut counts = member(
            "counts",
            Tref::Map(
                Box::new(Tref::Prim(Prim::I32)),
                Box::new(Tref::Prim(Prim::String)),
            ),
            true,
        );
        counts.traits = vec![crate::ir::Trait {
            id: "entries".into(),
            value: serde_json::json!(true),
        }];
        let module = Module {
            name: "models".into(),
            shapes: vec![structure("models#Doc", vec![counts])],
            operations: vec![],
            extensions: vec![],
        };
        let files = groups(&module);
        assert_eq!(files.len(), 2);

        // The Entry/Entries definitions and the typed field live in the types file,
        // with no imports and no (de)serialization methods.
        let types = rendered(&files, TYPES);
        assert!(types.contains("type Entry[K comparable, V any] struct {"));
        assert!(types.contains("type Entries[K comparable, V any] []Entry[K, V]"));
        assert!(types.contains("\tCounts Entries[int32, string] `json:\"counts\"`\n"));
        assert!(!types.contains("import "));
        assert!(!types.contains("MarshalJSON"));

        // The Entries methods live in the serde file, which pulls encoding/json; with
        // no union there is no marshalVariant.
        let serde = rendered(&files, INTERNAL);
        assert!(serde.contains("func (e Entries[K, V]) MarshalJSON() ([]byte, error) {"));
        assert!(serde.contains("func (e *Entries[K, V]) UnmarshalJSON(b []byte) error {"));
        assert!(serde.contains("import \"encoding/json\""));
        assert!(!serde.contains("func marshalVariant("));
    }
}

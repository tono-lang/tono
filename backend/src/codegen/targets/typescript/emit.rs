//! Assembling a TypeScript module from an IR module into separate output files so
//! the types read without the serialization noise: a types file (the branded
//! well-known aliases, each interface, each open-enum literal union, and each
//! discriminated union) and, when there is anything to serialize, a serde file
//! (the shared codec runtime helpers and each shape's `encode`/`decode`). The two
//! are separate TypeScript modules, so the serde file imports the types it
//! references from the types file (`imports_companion`); the helpers depend only on
//! built-ins, so a module of plain JSON-native types still gets only a types file.

use crate::codegen::casing::CasingConfig;
use crate::codegen::group::Group;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::client;
use crate::codegen::targets::typescript::codecs::{
    emit_codecs, runtime_helpers, RUNTIME_HELPER_NAMES,
};
use crate::codegen::targets::typescript::errors;
use crate::codegen::targets::typescript::types::{emit_type, emit_validators};
use crate::codegen::tree::{Alias, Decl, FnBody, ModuleFile};
use crate::codegen::validation;
use crate::codegen::visibility::Exposed;
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

/// The SDK-root group's declarations: the codec runtime helpers every module's
/// codecs call. They serve no module in particular, so the whole SDK carries one
/// copy instead of one per module.
pub fn shared_decls() -> Vec<Decl> {
    runtime_helpers()
}

/// The text of a declaration that carries opaque source, or `None` for one the
/// tree models structurally.
fn raw_text(decl: &Decl) -> Option<&str> {
    match decl {
        Decl::Function(f) => {
            let FnBody::Raw { text, .. } = &f.body;
            Some(text)
        }
        Decl::Raw(raw) => Some(&raw.text),
        _ => None,
    }
}

/// The top-level names a group declares through opaque text. The tree cannot be
/// read for them (that is what makes the text opaque), so they are recovered
/// from it: an exported declaration is a line starting with `export` and naming
/// what it declares.
fn exported_in_text(decls: &[Decl]) -> Vec<String> {
    let mut names = Vec::new();
    for text in decls.iter().filter_map(raw_text) {
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("export ") else {
                continue;
            };
            let mut words = rest.split_whitespace();
            let (Some(keyword), Some(name)) = (words.next(), words.next()) else {
                continue;
            };
            if !matches!(
                keyword,
                "function" | "const" | "class" | "interface" | "type" | "abstract"
            ) {
                continue;
            }
            let name: String = name
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !name.is_empty() {
                names.push(name);
            }
        }
    }
    names
}

/// Declare the symbols a group's declarations call from another group, so the
/// engine collects the import. A raw body names a helper or a codec in text
/// rather than through a symbol, so the reference is recovered from the text: a
/// name is referenced exactly when it occurs in one of them.
fn attach_text_refs(decls: &mut [Decl], names: &[(String, String)]) {
    let used: Vec<Symbol> = names
        .iter()
        .filter(|(name, _)| {
            decls
                .iter()
                .filter_map(raw_text)
                .any(|text| text.contains(name.as_str()))
        })
        .map(|(name, module)| Symbol::imported(name.clone(), module.clone(), name.clone()))
        .collect();
    if used.is_empty() {
        return;
    }
    if let Some(Decl::Function(first)) = decls.iter_mut().find(|d| matches!(d, Decl::Function(_))) {
        let FnBody::Raw { refs, .. } = &mut first.body;
        refs.extend(used);
    } else if let Some(Decl::Raw(first)) = decls.iter_mut().find(|d| matches!(d, Decl::Raw(_))) {
        first.refs.extend(used);
    }
}

/// The shared runtime helpers, paired with the group that declares them.
fn runtime_helper_refs() -> Vec<(String, String)> {
    RUNTIME_HELPER_NAMES
        .iter()
        .map(|name| ((*name).to_string(), crate::codegen::group::ROOT.to_string()))
        .collect()
}

/// Assemble a TypeScript module into separate output files: a types file (the
/// branded well-known aliases and each shape's type declaration) and, when there is
/// anything to serialize, a serde file (the runtime helpers and each shape's
/// codecs). The serde file is a separate module, so it imports the module's types
/// from the types file; the runtime helpers depend only on built-ins. A module of
/// plain JSON-native types still always has codecs, so both files are emitted in
/// practice, but the serde file is omitted when no codec is produced.
pub fn emit_module(module: &Module, config: &CasingConfig, exposed: &Exposed) -> Vec<ModuleFile> {
    let mut type_decls = Vec::new();
    let mut codec_decls = Vec::new();
    // A shape a public type reaches is public; the rest are the module's own
    // business and move to its internal group, taking their codecs with them.
    let mut internal_decls = Vec::new();
    for shape in &module.shapes {
        let mut types = emit_type(shape, config);
        // Validators live with the type they check.
        types.extend(emit_validators(shape, config));
        let codecs = emit_codecs(shape, config, &module.name);
        if exposed.shape(shape) {
            type_decls.extend(types);
            codec_decls.extend(codecs);
        } else {
            internal_decls.extend(types);
            internal_decls.extend(codecs);
        }
    }
    let module_has_entries = crate::codegen::entries::has_entries(module);
    // Operations bring the error classes and the client interface into the
    // types file and the discriminators in with the codecs they call.
    if !module.operations.is_empty() {
        type_decls.extend(errors::type_decls(module, config));
        codec_decls.extend(errors::serde_decls(module));
        // The transport client lives with the codecs it calls (encode input,
        // decode output, the error discriminator) and embeds each operation's
        // opaque wire descriptor.
        codec_decls.extend(client::client_decls(module, config));
    } else if module_has_entries {
        // An entry's client maps outcomes onto the same taxonomy; its client
        // surface is its own exported class, so the loose-op interface (and
        // the generic HttpClient) stays out.
        type_decls.extend(errors::taxonomy_and_declared_decls(module));
    } else if module.shapes.iter().any(validation::shape_has_checks) {
        // Constraints without operations still need the Validation category a
        // validator returns (root, `Violation`, `ValidationError`), which the
        // taxonomy would otherwise have carried.
        type_decls.extend(errors::standalone_validation_decls());
    }
    let entries = crate::codegen::targets::typescript::entry::emit(module, config);
    // The entry's shared machinery names the module's own types, so it rides the
    // codec group beside them rather than the group that moves away.
    codec_decls.extend(entries.shared);
    attach_text_refs(&mut codec_decls, &runtime_helper_refs());
    attach_text_refs(&mut internal_decls, &runtime_helper_refs());

    // What the codec group declares as opaque text (the codecs, the resolution
    // helpers, the bound-hook wrappers) is what an entry group calls, so those
    // names are what its imports are recovered from.
    let codec_names: Vec<(String, String)> = exported_in_text(&codec_decls)
        .into_iter()
        .chain(crate::codegen::imports::declared_symbols(&codec_decls))
        .map(|name| (name, module.name.clone()))
        .chain(runtime_helper_refs())
        .collect();

    let mut files = vec![ModuleFile::new(Group::types(&module.name), type_decls)];
    // One group per entry declaration, named after it: the entry's class, its
    // Settings, and its operation methods, so the construction surface reads
    // together instead of riding the file named for serialization.
    for (name, mut decls) in entries.per_entry {
        attach_text_refs(&mut decls, &codec_names);
        let provides = exported_in_text(&decls);
        files.push(ModuleFile::new(Group::entry(&module.name, &name), decls).providing(provides));
    }
    if !codec_decls.is_empty() {
        let provides = exported_in_text(&codec_decls);
        files.push(ModuleFile::new(Group::codec(&module.name), codec_decls).providing(provides));
    }
    if !internal_decls.is_empty() {
        let provides = exported_in_text(&internal_decls);
        files.push(
            ModuleFile::new(Group::module_internal(&module.name), internal_decls)
                .providing(provides),
        );
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::group::{CODEC, TYPES};
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::ir::{Member, Prim, Shape, ShapeKind, Tref};

    /// Emit a module's groups with everything exposed, resolved the way the
    /// pipeline resolves them.
    fn groups(module: &Module) -> Vec<ModuleFile> {
        crate::codegen::test_support::resolve_groups(emit_module(
            module,
            &ts_casing(),
            &Exposed::all(),
        ))
    }

    /// Render the named group of a module, panicking if it did not emit one.
    fn rendered(files: &[ModuleFile], group: &str) -> String {
        crate::codegen::test_support::render_group(
            files,
            group,
            crate::codegen::TargetKind::TypeScript,
            &TsRules,
        )
    }

    #[test]
    fn constraints_without_operations_still_carry_the_validation_category() {
        // No operation means no taxonomy, but the validator returns the Validation
        // category, so its classes must still be emitted or the module cannot compile.
        let module = crate::codegen::test_support::constrained_module();
        let types = rendered(&groups(&module), TYPES);
        assert!(types.contains("export interface Violation {"));
        assert!(types.contains("export class ValidationError extends TonoError {"));
        // The category extends the root, so the root rides along with it.
        assert!(types.contains("export abstract class TonoError extends Error {"));
        assert!(types
            .contains("export function validateCharge(value: Charge): ValidationError | null {"));
        // Only the category the validator needs, not the rest of the taxonomy.
        assert!(!types.contains("export class TransportError"));
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
            extensions: vec![],
        };
        let files = groups(&module);
        assert_eq!(files.len(), 2, "TypeScript splits types from serde");

        // The types file holds the interface, with no codec and no runtime
        // helper; the branded aliases belong to the SDK's support group.
        let types = rendered(&files, TYPES);
        assert!(!types.contains("export type Timestamp = string"));
        assert!(types.contains("export interface Charge {"));
        assert!(types.contains("  amountCents: bigint;"));
        assert!(!types.contains("export function encodeI64"));
        assert!(!types.contains("export function encodeCharge"));
        assert!(!types.contains("import "));

        // The serde file holds the runtime helpers and the codecs, and imports the
        // types it references from the types file.
        let serde = rendered(&files, CODEC);
        assert!(serde.contains("import { Charge } from \"./types\";"));
        // The runtime helpers are the SDK's, not the module's, so they are
        // imported rather than repeated here.
        assert!(serde.contains("import { decodeI64, encodeI64 } from \"../internal\";"));
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
            extensions: vec![],
        };
        let files = groups(&module);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].group.name, TYPES);
    }
}

//! Generation-time validation of the entry surface (name-collision and
//! target-rule checks the frontend cannot see). Split out of `entries/mod.rs`
//! to stay under this repo's per-file line ceiling.

use super::{
    arg_identifiers, companion_name, has_source, local_name, module_entries, op_local_name,
};
use crate::ir::{ShapeKind, Source, Tref};

/// The construction-surface names the generated Settings reserves for its
/// transport slots, per binding language key. An entry field with one of
/// these canonical names would collide with the slot member.
const RESERVED_SLOT_FIELDS: [&str; 4] = ["headers", "transport", "http_client", "fetch"];

/// Canonical names an `@arg` field cannot take: they become bare constructor
/// parameters, and these are the locals and parameters the generated
/// constructors already declare (Go's Settings/carrier/values plumbing, the
/// TypeScript config object, both targets' scratch variables).
const RESERVED_ARG_NAMES: [&str; 18] = [
    "s",
    "w",
    "v",
    "n",
    "ok",
    "raw",
    "err",
    "ms",
    "opt",
    "opts",
    "config",
    "values",
    "violations",
    "runtime",
    "probe",
    "dec",
    "decoded",
    "composed",
];

/// Language keywords an `@arg` field cannot take either: a single-word
/// canonical name passes through the camel casing unchanged, so a Go or
/// TypeScript keyword would land verbatim as a parameter name.
const RESERVED_ARG_KEYWORDS: [&str; 43] = [
    "break",
    "case",
    "catch",
    "chan",
    "class",
    "const",
    "continue",
    "default",
    "defer",
    "delete",
    "do",
    "else",
    "enum",
    "fallthrough",
    "finally",
    "for",
    "func",
    "function",
    "go",
    "goto",
    "if",
    "import",
    "in",
    "instanceof",
    "interface",
    "let",
    "map",
    "new",
    "package",
    "range",
    "return",
    "select",
    "static",
    "struct",
    "super",
    "switch",
    "this",
    "throw",
    "try",
    "type",
    "typeof",
    "var",
    "void",
];

/// The Pascal spelling of a canonical name, for collision checks against the
/// generated type surface.
fn pascal_ident(name: &str) -> String {
    crate::codegen::casing::transform(
        name,
        crate::codegen::symbol::SymbolKind::Type,
        &crate::codegen::casing::CasingConfig::new(crate::codegen::casing::CaseStyle::Pascal),
        None,
    )
}

/// Generation-time validation of the entry surface: the cases the frontend
/// cannot see (they are target rules) and that would otherwise produce
/// uncompilable or silently wrong output. Returns the first offense.
///
/// This also covers the one loose-operation rule: entries are the only
/// supported HTTP client surface, so a loose (non-entry) operation carrying a
/// `wire` binding is rejected here, the same way an entry `@http` op with no
/// endpoint is below. The frontend still accepts a loose `@http` op (nothing
/// about the language changes), so this is a generation-time gap the same as
/// the endpoint check, not a checker rule.
pub fn validate_entries(model: &crate::ir::Model) -> Result<(), String> {
    for module in &model.modules {
        for op in &module.operations {
            if let ShapeKind::Operation { wire: Some(_), .. } = &op.kind {
                return Err(format!(
                    "module {}: operation {} carries an @http binding outside an entry; \
                     entries are the only supported HTTP client surface",
                    module.name,
                    local_name(&op.id)
                ));
            }
        }
        let entries = module_entries(module);
        if entries.is_empty() {
            continue;
        }
        for entry in &entries {
            // The frontend requires an entry @http operation to name its
            // endpoint, but the IR is also accepted straight from a file or
            // stdin, so the gap surfaces here as a clean generation error
            // instead of a panic inside an emitter.
            for op in entry.operations {
                if let ShapeKind::Operation {
                    wire: Some(wire), ..
                } = &op.kind
                {
                    if wire.endpoint.is_none() {
                        return Err(format!(
                            "module {}: entry {} operation {} carries an @http binding with no endpoint; an entry operation's @http must name its endpoint",
                            module.name,
                            entry.name,
                            local_name(&op.id)
                        ));
                    }
                }
            }
            let declared = entry.declared();
            for field in declared.iter().copied() {
                if RESERVED_SLOT_FIELDS.contains(&field.name.as_str()) {
                    return Err(format!(
                        "module {}: entry {} field {} collides with the generated Settings transport slot of the same name; rename the field",
                        module.name, entry.name, field.name
                    ));
                }
                if has_source(field, |s| matches!(s, Source::Arg)) {
                    // The generated @arg parameter is the canonical name cased, or
                    // its @rename(lang) override verbatim: every spelling the
                    // parameter can take must clear the reserved lists, not just
                    // the canonical one.
                    for candidate in arg_identifiers(field) {
                        if RESERVED_ARG_NAMES.contains(&candidate.as_str()) {
                            return Err(format!(
                                "module {}: entry {} field {} is an @arg but its generated parameter {} is a local the generated constructor already declares; rename the field or its @rename",
                                module.name, entry.name, field.name, candidate
                            ));
                        }
                        if RESERVED_ARG_KEYWORDS.contains(&candidate.as_str()) {
                            return Err(format!(
                                "module {}: entry {} field {} is an @arg but its generated parameter {} is a keyword in a target language; rename the field or its @rename",
                                module.name, entry.name, field.name, candidate
                            ));
                        }
                    }
                }
                // The generated resolution derives a `<field>_why` reason
                // variable and a `<field>_set` flag per field; a sibling
                // field spelling either name collides with them.
                for suffix in ["_why", "_set"] {
                    let derived = format!("{}{suffix}", field.name);
                    if declared.iter().any(|other| other.name == derived) {
                        return Err(format!(
                            "module {}: entry {} declares both {} and {}; the resolution derives a variable named {} for the former, rename one",
                            module.name, entry.name, field.name, derived, derived
                        ));
                    }
                }
                // A construction field resolves within its module: the
                // resolution idiom (config vs structured vs scalar) and the
                // generated type surface both need the referenced shape.
                if let Tref::Ref { id, .. } = &field.target {
                    if !module.shapes.iter().any(|shape| shape.id == *id) {
                        return Err(format!(
                            "module {}: entry {} field {} references {}, a shape outside this module; construction fields resolve within their module, move the shape or the entry",
                            module.name, entry.name, field.name, id
                        ));
                    }
                }
            }
        }
        // A module with loose operations emits the `Client` interface; an
        // entry named `client` would emit a same-named concrete type next to
        // it.
        if !module.operations.is_empty() && entries.iter().any(|e| e.name == "client") {
            return Err(format!(
                "module {}: the entry client collides with the Client interface the module's loose operations emit; rename the entry or move the loose operations into it",
                module.name
            ));
        }
        // client_init bridges one Settings type; with several entries the
        // bespoke symbol cannot have both signatures, and skipping it
        // silently would drop declared behavior. Only the languages whose
        // targets emit the bridge count: a binding for another language does
        // not block this generation.
        let client_init_bound = module.extensions.iter().any(|e| {
            e.kind == crate::ir::ExtKind::Hook
                && e.name == "client_init"
                && ["go", "ts", "typescript"]
                    .iter()
                    .any(|lang| e.bindings.contains_key(*lang))
        });
        if entries.len() > 1 && client_init_bound {
            return Err(format!(
                "module {}: client_init is bound but the module declares {} entries; the hook bridges one Settings type, so keep one entry per module (or drop the binding)",
                module.name,
                entries.len()
            ));
        }
        // In a mixed module the loose-op TypeScript client already owns the
        // client_init wrapper (with its own signature); two bridges cannot
        // share one bespoke symbol.
        let ts_client_init = module.extensions.iter().any(|e| {
            e.kind == crate::ir::ExtKind::Hook
                && e.name == "client_init"
                && (e.bindings.contains_key("ts") || e.bindings.contains_key("typescript"))
        });
        if !module.operations.is_empty() && ts_client_init {
            return Err(format!(
                "module {}: client_init is bound for TypeScript but the module mixes loose operations with an entry; move the loose operations into the entry (the loose client and the entry bridge cannot share the hook)",
                module.name
            ));
        }
        // An entry named after another entry's generated companion (its
        // Settings/Config/Option/API types) would emit two same-named types.
        let mut companions: Vec<(String, &str)> = Vec::new();
        let multi = entries.len() > 1;
        for entry in &entries {
            companions.push((companion_name(entry.name, "settings", multi), entry.name));
            companions.push((format!("{}_config", entry.name), entry.name));
            companions.push((format!("{}_option", entry.name), entry.name));
            companions.push((format!("{}_api", entry.name), entry.name));
        }
        for entry in &entries {
            if let Some((companion, owner)) = companions.iter().find(|(c, _)| *c == entry.name) {
                return Err(format!(
                    "module {}: entry {} collides with the {} companion generated for entry {}; rename the entry",
                    module.name, entry.name, companion, owner
                ));
            }
        }
        // A declared shape spelling a generated type (an entry's client type
        // or one of its companions) would emit two same-named types.
        let mut generated: Vec<(String, String)> = Vec::new();
        for entry in &entries {
            generated.push((
                pascal_ident(entry.name),
                format!("client type of entry {}", entry.name),
            ));
        }
        for (companion, owner) in &companions {
            generated.push((
                pascal_ident(companion),
                format!("{companion} companion of entry {owner}"),
            ));
        }
        for shape in &module.shapes {
            if matches!(shape.kind, ShapeKind::Entry { .. }) {
                continue;
            }
            let ident = local_name(&shape.id);
            if let Some((_, what)) = generated.iter().find(|(g, _)| g == ident) {
                return Err(format!(
                    "module {}: shape {} collides with the {}; rename the shape",
                    module.name, ident, what
                ));
            }
        }
        // The Go constructor is `New` (single entry) or `New<Entry>` (multi):
        // an entry type spelling the same identifier cannot share the package.
        if !multi && entries.iter().any(|e| e.name == "new") {
            return Err(format!(
                "module {}: the entry new collides with the New constructor it generates; rename the entry",
                module.name
            ));
        }
        if multi {
            for entry in &entries {
                if let Some(other) = entries
                    .iter()
                    .find(|o| entry.name == format!("new_{}", o.name))
                {
                    return Err(format!(
                        "module {}: entry {} collides with the New{} constructor generated for entry {}; rename one",
                        module.name,
                        entry.name,
                        pascal_ident(other.name),
                        other.name
                    ));
                }
            }
        }
        // A loose op and an entry op sharing a local name would emit two
        // descriptors/discriminators under the same generated identifier.
        for entry in &entries {
            for op in entry.operations {
                let local = op_local_name(&op.id);
                if module
                    .operations
                    .iter()
                    .any(|loose| local_name(&loose.id) == local)
                {
                    return Err(format!(
                        "module {}: operation {} is declared both loose and in entry {}; the generated companions would collide, rename one",
                        module.name, local, entry.name
                    ));
                }
            }
        }
    }
    Ok(())
}

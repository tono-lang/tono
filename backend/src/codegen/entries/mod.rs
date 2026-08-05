//! The language-neutral entry model every target consumes to emit the SDK
//! construction surface: which fields the constructor takes (`@arg`), which
//! ride the options surface (`@with`), the order the resolution chain runs in,
//! and the resolved-value entries the runtime looks up by canonical name.
//!
//! All of it is derived here exactly once so the same entry constructs
//! identically across every generated SDK: a target never re-reads the sources
//! itself, it only spells the steps in its own idiom. The frontend has already
//! typechecked the resolution DAG (cycles, dead sources, exhaustiveness), so
//! this model trusts the IR and never reports.

use std::collections::HashSet;

use crate::ir::{
    ArmValue, EntryField, Member, Module, Shape, ShapeKind, Source, TemplatePart, Tref,
};

mod checks;
mod order;
pub mod plan;

pub use checks::{needs_presence_guard, value_path_access, value_path_frozen_expr};
use order::resolution_order;

/// An entry field seen as a required struct member, so the declared-validation
/// guard machinery (shared with structure members) can run over it.
pub(crate) fn field_as_member(field: &EntryField) -> Member {
    Member {
        name: field.name.clone(),
        target: field.target.clone(),
        required: true,
        default: None,
        constraints: field.constraints.clone(),
        traits: field.traits.clone(),
    }
}

/// One entry of a module with its fields in resolution order: every field a
/// step depends on (a `@env(.ref)` variable name, a `@format` placeholder, a
/// match subject or arm, a `@bind` source) is resolved before it.
pub struct EntryModel<'a> {
    pub shape: &'a Shape,
    /// The canonical (snake) local name.
    pub name: &'a str,
    /// Fields in dependency (resolution) order.
    pub fields: Vec<&'a EntryField>,
    /// The operations nested in the entry body, descriptors attached.
    pub operations: &'a [Shape],
}

/// What kind of value a field's declared type resolves to, which decides the
/// resolution idiom: a scalar parsed at the boundary, a construction-only
/// config composed via `@bind`, or a wire structure decoded strictly from a
/// structured source (JSON in an env variable).
pub enum FieldShape<'a> {
    Scalar,
    Config(&'a Shape),
    Structured(&'a Shape),
    /// A map/list decoded as JSON whole; no per-member layering.
    Json,
}

/// The local (snake) name of a shape id (`notes#client` -> `client`).
pub fn local_name(id: &str) -> &str {
    id.rsplit('#').next().unwrap_or(id)
}

/// Every entry of a module, fields already in resolution order.
pub fn module_entries<'a>(module: &'a Module) -> Vec<EntryModel<'a>> {
    module
        .shapes
        .iter()
        .filter_map(|shape| match &shape.kind {
            ShapeKind::Entry { fields, operations } => Some(EntryModel {
                shape,
                name: local_name(&shape.id),
                fields: resolution_order(fields, module),
                operations,
            }),
            _ => None,
        })
        .collect()
}

/// Whether the module declares any entry (drives the error taxonomy and the
/// serde file the entry client rides in).
pub fn has_entries(module: &Module) -> bool {
    module
        .shapes
        .iter()
        .any(|s| matches!(s.kind, ShapeKind::Entry { .. }))
}

impl<'a> EntryModel<'a> {
    /// The constructor's positional parameters: the `@arg` fields, in
    /// declaration order (the resolution order never reorders a signature).
    pub fn args(&self) -> Vec<&'a EntryField> {
        self.declared()
            .into_iter()
            .filter(|f| has_source(f, |s| matches!(s, Source::Arg)))
            .collect()
    }

    /// The configurable fields: the `@with` fields, in declaration order.
    pub fn with_fields(&self) -> Vec<&'a EntryField> {
        self.declared()
            .into_iter()
            .filter(|f| has_source(f, |s| matches!(s, Source::With)))
            .collect()
    }

    /// Fields in declaration order (as written in the IR). The generated
    /// public surface (Settings, config objects) follows this order; only the
    /// constructor body follows the resolution order.
    pub fn declared(&self) -> Vec<&'a EntryField> {
        let ShapeKind::Entry { fields, .. } = &self.shape.kind else {
            return Vec::new();
        };
        fields.iter().collect()
    }

    /// The field shape a target dispatches its resolution idiom on.
    pub fn field_shape(&self, field: &EntryField, module: &'a Module) -> FieldShape<'a> {
        match &field.target {
            Tref::Ref { id, .. } => match module.shapes.iter().find(|s| s.id == *id) {
                Some(shape) if matches!(shape.kind, ShapeKind::Config { .. }) => {
                    FieldShape::Config(shape)
                }
                Some(shape) if matches!(shape.kind, ShapeKind::Structure { .. }) => {
                    FieldShape::Structured(shape)
                }
                // An enum is a branded string (always open), so the boundary
                // takes the raw value with a cast, like any other scalar.
                Some(shape) if matches!(shape.kind, ShapeKind::Enum { .. }) => FieldShape::Scalar,
                _ => FieldShape::Json,
            },
            Tref::Map(_, _) | Tref::List(_) => FieldShape::Json,
            _ => FieldShape::Scalar,
        }
    }

    /// The resolved-value entries the generated client hands the runtime as
    /// `Options.Values`: every field under its canonical dotted name, plus one
    /// entry per member of a composed config or structured field (the paths a
    /// descriptor's ref positions can name).
    pub fn value_paths(&self, module: &'a Module) -> Vec<ValuePath<'a>> {
        let mut out = Vec::new();
        for field in &self.fields {
            out.push(ValuePath {
                path: field.name.clone(),
                field,
                member: None,
                target: &field.target,
            });
            if let Tref::Ref { id, .. } = &field.target {
                if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                    let members: Vec<(&'a str, &'a Tref)> = match &shape.kind {
                        ShapeKind::Config { fields } => fields
                            .iter()
                            .map(|f| (f.name.as_str(), &f.target))
                            .collect(),
                        ShapeKind::Structure { members, .. } => members
                            .iter()
                            .map(|m| (m.name.as_str(), &m.target))
                            .collect(),
                        _ => Vec::new(),
                    };
                    for (member, target) in members {
                        out.push(ValuePath {
                            path: format!("{}.{member}", field.name),
                            field,
                            member: Some(member.to_string()),
                            target,
                        });
                    }
                }
            }
        }
        out
    }
}

impl<'a> EntryModel<'a> {
    /// The declared type a sibling-field path resolves to: the field's own
    /// type for a bare name, the member's type when the path reaches into a
    /// composed or structured field. Unresolvable paths (the frontend rejects
    /// them) read as strings so the emitters stay total.
    pub fn path_type(&self, path: &[String], module: &Module) -> Tref {
        let head = self
            .fields
            .iter()
            .find(|f| path.first().is_some_and(|h| *h == f.name));
        let Some(head) = head else {
            return Tref::Prim(crate::ir::Prim::String);
        };
        if path.len() == 1 {
            return head.target.clone();
        }
        if let Tref::Ref { id, .. } = &head.target {
            if let Some(shape) = module.shapes.iter().find(|s| s.id == *id) {
                let target = match &shape.kind {
                    ShapeKind::Config { fields } => fields
                        .iter()
                        .find(|f| f.name == path[1])
                        .map(|f| f.target.clone()),
                    ShapeKind::Structure { members, .. } => members
                        .iter()
                        .find(|m| m.name == path[1])
                        .map(|m| m.target.clone()),
                    _ => None,
                };
                if let Some(t) = target {
                    return t;
                }
            }
        }
        Tref::Prim(crate::ir::Prim::String)
    }

    /// [`Self::is_guaranteed`] looked up by field name.
    pub fn field_guaranteed(&self, name: &str) -> bool {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .is_some_and(|f| self.is_guaranteed(f))
    }

    /// The `@rename(lang)` override of the entry field with this canonical name,
    /// so its generated identifier (a constructor param, a `With*`/`with` option,
    /// a Settings member, and every internal reference) reads idiomatically in
    /// the target without changing the canonical name the runtime looks up by.
    pub fn field_rename(&self, name: &str, lang: &str) -> Option<String> {
        self.fields
            .iter()
            .find(|f| f.name == name)
            .and_then(|f| crate::codegen::conventions::rename_of(&f.traits, lang))
    }
}

/// Whether a leaf type is an enum reference (a branded string on the wire),
/// which is what lets it freeze into the runtime values wherever it sits (a
/// field or a composed/structured member).
pub fn ref_is_enum(t: &Tref, module: &Module) -> bool {
    let Tref::Ref { id, .. } = t else {
        return false;
    };
    module
        .shapes
        .iter()
        .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Enum { .. }))
}

/// A bare copy of a field carrying only a source chain: what a match arm's
/// inline sources and a composed member's own chain resolve as.
pub fn source_stub(field: &EntryField, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: field.name.clone(),
        target: field.target.clone(),
        sources,
        format: None,
        transforms: vec![],
        select: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// One resolved-value entry: its canonical dotted path, the entry field it
/// hangs off, and the member name when the path reaches into a composed or
/// structured field.
pub struct ValuePath<'a> {
    pub path: String,
    pub field: &'a EntryField,
    pub member: Option<String>,
    /// The declared type at the path's leaf (the member's own type when the
    /// path reaches into a composed/structured field).
    pub target: &'a Tref,
}

/// The bare operation name of an entry-nested op: the id is
/// `module#entry.op`, so the local part still carries the entry prefix the
/// generated method name must not.
pub fn op_local_name(id: &str) -> &str {
    let local = local_name(id);
    local.rsplit('.').next().unwrap_or(local)
}

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
pub fn validate_entries(model: &crate::ir::Model) -> Result<(), String> {
    for module in &model.modules {
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

/// The base (canonical snake) name of a per-entry companion declaration:
/// unprefixed for a module with a single entry, prefixed by the entry's name
/// when several entries share the module (a target rule, invisible in the
/// DSL; it keeps `New`/`Settings`/`With*` unambiguous only where they would
/// actually collide).
pub fn companion_name(entry: &str, base: &str, multi: bool) -> String {
    if multi {
        format!("{entry}_{base}")
    } else {
        base.to_string()
    }
}

fn has_source(field: &EntryField, pred: impl Fn(&Source) -> bool) -> bool {
    field.sources.iter().any(pred)
}

/// Every spelling an `@arg` field's generated parameter can take: the canonical
/// name (the reserved lists are single-word, so casing does not change them) and
/// each `@rename(lang)` override, which is used verbatim. The collision checks
/// clear all of them, not just the canonical name.
fn arg_identifiers(field: &EntryField) -> Vec<String> {
    let mut out = vec![field.name.clone()];
    if let Some(t) = crate::codegen::conventions::rename_map(&field.traits) {
        if let Some(map) = t.value.as_object() {
            for v in map.values() {
                if let Some(s) = v.as_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
    out
}

impl<'a> EntryModel<'a> {
    /// Whether a field always resolves to a value: an `@arg`, a chain ending in
    /// `@default`, or a derivation whose inputs are all guaranteed. A
    /// non-guaranteed field carries a "why absent" reason through the generated
    /// resolution so a consumer can report the chain at its point of use.
    pub fn is_guaranteed(&self, field: &EntryField) -> bool {
        let mut visiting = HashSet::new();
        self.guaranteed_inner(field, &mut visiting)
    }

    fn guaranteed_inner(&self, field: &EntryField, visiting: &mut HashSet<&'a str>) -> bool {
        // A field currently being resolved is a cycle the frontend already
        // rejected; treat it as not guaranteed rather than recursing forever.
        let Some(field) = self.fields.iter().find(|f| f.name == field.name).copied() else {
            return false;
        };
        if !visiting.insert(field.name.as_str()) {
            return false;
        }
        let by_name = |name: &str| self.fields.iter().find(|f| f.name == name).copied();
        let path_guaranteed = |p: &[String], visiting: &mut HashSet<&'a str>| {
            p.first()
                .and_then(|head| by_name(head))
                .is_some_and(|f| self.guaranteed_inner(f, visiting))
        };
        // A source chain is guaranteed when it can always terminate: an `@arg`
        // is explicit and a `@default` is the last resort, while `@with` and
        // `@env` may simply not be there.
        let sources_guaranteed = |sources: &[Source]| {
            sources
                .iter()
                .any(|s| matches!(s, Source::Arg | Source::Default(_)))
        };
        let result = if let Some(select) = &field.select {
            path_guaranteed(&select.subject, visiting)
                && select.arms.iter().all(|arm| match &arm.value {
                    ArmValue::Lit(_) => true,
                    ArmValue::Field(p) => path_guaranteed(p, visiting),
                    ArmValue::Sources(sources) => sources_guaranteed(sources),
                })
        } else if let Some(format) = &field.format {
            format.iter().all(|part| match part {
                TemplatePart::Lit(_) => true,
                TemplatePart::Field(p) => path_guaranteed(p, visiting),
                // An op-input placeholder cannot appear in a field template
                // (the frontend rejects it); the emitters render it as an
                // empty literal, so the same classification keeps the
                // defensive output consistent (no absence to track).
                TemplatePart::Input(_) => true,
            })
        } else {
            sources_guaranteed(&field.sources)
        };
        visiting.remove(field.name.as_str());
        result
    }

    /// The head fields the entry's operations consume through their protocol
    /// traits, deduplicated ([`Self::consumed_field_paths`] carries the full
    /// paths).
    pub fn consumed_field_heads(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for path in self.consumed_field_paths() {
            if let Some(head) = path.first() {
                if !out.iter().any(|h| h == head) {
                    out.push(head.clone());
                }
            }
        }
        out
    }

    /// The field paths the entry's operations consume through their protocol
    /// traits: the `@http` endpoint reference, `{.field}` path placeholders,
    /// `@header` value references, and the `@timeout`/`@retry` references.
    /// These must hold a value once construction finishes (after `client_init`
    /// ran), so the constructor reports an absent one with its chain instead of
    /// letting the first call fail obscurely.
    pub fn consumed_field_paths(&self) -> Vec<Vec<String>> {
        let mut out: Vec<Vec<String>> = Vec::new();
        let mut push = |path: Option<Vec<String>>| {
            if let Some(path) = path {
                if !path.is_empty() && !out.contains(&path) {
                    out.push(path);
                }
            }
        };
        let field_path = |v: &serde_json::Value| -> Option<Vec<String>> {
            Some(
                v.get("field")?
                    .as_array()?
                    .iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect(),
            )
        };
        for op in self.operations {
            for t in &op.traits {
                match t.id.as_str() {
                    "http" => {
                        if let Some(path) = t.value.get("path").and_then(|p| p.as_str()) {
                            for placeholder in path_placeholder_paths(path) {
                                push(Some(placeholder));
                            }
                        }
                        if let Some(endpoint) = t.value.get("endpoint") {
                            push(field_path(endpoint));
                        }
                    }
                    "header" | "timeout" | "retry" => {
                        if let Some(items) = t.value.as_array() {
                            for item in items {
                                push(field_path(item));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        out
    }
}

/// The field path of every `{.a.b}` placeholder in a path template.
fn path_placeholder_paths(path: &str) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find("{.") {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let inner = &rest[start + 2..start + end];
        let segs: Vec<String> = inner
            .split('.')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        if !segs.is_empty() {
            out.push(segs);
        }
        rest = &rest[start + end + 1..];
    }
    out
}

#[cfg(test)]
mod tests;

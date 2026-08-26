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
    ArmValue, CallArg, EntryField, Member, Module, Shape, ShapeKind, Source, TemplatePart, Tref,
};

mod checks;
mod order;
pub mod plan;
pub(crate) mod spellings;
mod split;
mod validate;
mod validate_calls;
mod validate_ownership;
pub mod wire;

pub use checks::{needs_presence_guard, value_path_access, value_path_frozen_expr};
use order::resolution_order;
pub use split::{
    call_deps, has_wire_ops, is_foreign, model_has_wire_ops, ConstructionSplit, TailStep,
};
pub(crate) use validate::is_foreign_ref;
pub use validate::validate_entries;

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
    /// A `= ns.fn(args)` extern-call source, or a `= .field.method(args)`
    /// handle-method-call source. Its presentation is governed by the call,
    /// not by its target type's own shape (which may be a plain struct or an
    /// opaque handle with no entry in `module.shapes` at all).
    Call,
}

/// The local (snake) name of a shape id (`notes#client` -> `client`).
pub fn local_name(id: &str) -> &str {
    id.rsplit('#').next().unwrap_or(id)
}

/// The name every target generates the module's own type `canonical` (a
/// shape's local name, what a foreign spelling references as
/// `.canonical`) under; `None` when the module declares no such type.
/// Type names are PascalCase in all three targets, so the rendering is
/// the same everywhere a spelling is emitted.
pub fn generated_type_name(module: &Module, canonical: &str) -> Option<String> {
    module
        .shapes
        .iter()
        .find(|s| local_name(&s.id) == canonical)
        .map(|s| {
            crate::codegen::casing::transform(
                local_name(&s.id),
                crate::codegen::symbol::SymbolKind::Type,
                &crate::codegen::casing::CasingConfig::new(
                    crate::codegen::casing::CaseStyle::Pascal,
                ),
                None,
            )
        })
}

/// The resolver a target hands `foreign_spelling::qualify` for the
/// generated-type references of `module`'s spellings. A reference no
/// shape answers is refused before any emitter runs
/// (`validate_calls::spelling_references_resolve`), so reaching one here
/// is a generator bug.
pub fn generated_type(module: &Module) -> impl Fn(&str) -> String + '_ {
    move |name| {
        generated_type_name(module, name).unwrap_or_else(|| {
            panic!(
                "a foreign spelling references .{name}, which module {} does not declare; validate_calls::spelling_references_resolve should have refused it",
                module.name
            )
        })
    }
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
        if field.call.is_some() || field.handle_call.is_some() {
            return FieldShape::Call;
        }
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
        call: None,
        handle_call: None,
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

/// Whether every ref a call's arguments read is itself guaranteed, recursing
/// into a ctor field's values, a list's items, and a nested call's own args.
/// A `Param`/`Lit` reads nothing, so it is trivially guaranteed.
fn call_args_guaranteed<'a>(
    args: &[CallArg],
    path_guaranteed: &impl Fn(&[String], &mut HashSet<&'a str>) -> bool,
    visiting: &mut HashSet<&'a str>,
) -> bool {
    args.iter().all(|arg| match arg {
        CallArg::Ref(path) => path_guaranteed(path, visiting),
        CallArg::Ctor(ctor) => ctor
            .fields
            .values()
            .all(|v| call_args_guaranteed(std::slice::from_ref(v), path_guaranteed, visiting)),
        CallArg::List(items) => call_args_guaranteed(items, path_guaranteed, visiting),
        CallArg::Call(call) => call_args_guaranteed(&call.args, path_guaranteed, visiting),
        CallArg::SymbolCall(sc) => call_args_guaranteed(&sc.args, path_guaranteed, visiting),
        CallArg::Param(_)
        | CallArg::ParamAs { .. }
        | CallArg::Foreign(_)
        | CallArg::Lit(_)
        | CallArg::TypeRef(_) => true,
    })
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
                    // Guaranteed exactly when the subject itself is (already
                    // required by the `path_guaranteed(&select.subject, ..)`
                    // conjunct above).
                    ArmValue::Subject => true,
                })
        } else if let Some(format) = &field.format {
            format.iter().all(|part| match part {
                TemplatePart::Lit(_) => true,
                TemplatePart::Field(p) => path_guaranteed(p, visiting),
                // An op-input or op-parameter placeholder cannot appear in a
                // field template (the frontend rejects it); the emitters
                // render it as an empty literal, so the same classification
                // keeps the defensive output consistent (no absence to
                // track).
                TemplatePart::Input(_) | TemplatePart::Param(_) => true,
            })
        } else if let Some(call) = &field.call {
            // A call always attempts and either resolves or fails construction
            // outright (a ContractError boundary, not an absence a downstream
            // chain falls back past); it is guaranteed once every ref it reads
            // is. An `@with` alongside it only adds an injected
            // shortcut ahead of the same fallback call, so the classification
            // reduces to the same check either way: whether the call's own
            // reads are guaranteed. `field.sources` plays no part here.
            call_args_guaranteed(&call.args, &path_guaranteed, visiting)
        } else if let Some(call) = &field.handle_call {
            // Same regime as a free call, plus the receiver: the handle it
            // reads must itself be guaranteed for the call to run at all.
            path_guaranteed(&call.recv, visiting)
                && call_args_guaranteed(&call.args, &path_guaranteed, visiting)
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
    /// These must hold a value once construction finishes, so the constructor
    /// reports an absent one with its chain instead of letting the first call
    /// fail obscurely.
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

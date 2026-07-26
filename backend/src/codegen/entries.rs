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
    ArmValue, Bind, EntryField, EnvName, Module, Shape, ShapeKind, Source, TemplatePart, Tref,
};

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
                fields: resolution_order(fields),
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
    pub fn withs(&self) -> Vec<&'a EntryField> {
        self.declared()
            .into_iter()
            .filter(|f| has_source(f, |s| matches!(s, Source::With)))
            .collect()
    }

    /// Fields in declaration order (as written in the IR).
    fn declared(&self) -> Vec<&'a EntryField> {
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
                TemplatePart::Input(_) => false,
            })
        } else {
            sources_guaranteed(&field.sources)
        };
        visiting.remove(field.name.as_str());
        result
    }

    /// The head fields the entry's operations consume through their protocol
    /// traits: the `@http` endpoint reference, `{.field}` path placeholders,
    /// `@header` value references, and the `@timeout`/`@retry` references.
    /// These must hold a value once construction finishes (after `client_init`
    /// ran), so the constructor reports an absent one with its chain instead of
    /// letting the first call fail obscurely.
    pub fn consumed_field_heads(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut push = |head: Option<&str>| {
            if let Some(head) = head {
                if !out.iter().any(|h| h == head) {
                    out.push(head.to_string());
                }
            }
        };
        let field_head = |v: &serde_json::Value| -> Option<String> {
            v.get("field")?
                .as_array()?
                .first()?
                .as_str()
                .map(String::from)
        };
        for op in self.operations {
            for t in &op.traits {
                match t.id.as_str() {
                    "http" => {
                        if let Some(path) = t.value.get("path").and_then(|p| p.as_str()) {
                            for head in path_placeholder_heads(path) {
                                push(Some(&head));
                            }
                        }
                        if let Some(endpoint) = t.value.get("endpoint") {
                            push(field_head(endpoint).as_deref());
                        }
                    }
                    "header" => {
                        if let Some(items) = t.value.as_array() {
                            for item in items {
                                push(field_head(item).as_deref());
                            }
                        }
                    }
                    "timeout" | "retry" => {
                        if let Some(items) = t.value.as_array() {
                            for item in items {
                                push(field_head(item).as_deref());
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

/// The head field of every `{.a.b}` placeholder in a path template.
fn path_placeholder_heads(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find("{.") {
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let inner = &rest[start + 2..start + end];
        if let Some(head) = inner.split('.').next() {
            if !head.is_empty() {
                out.push(head.to_string());
            }
        }
        rest = &rest[start + end + 1..];
    }
    out
}

/// The sibling fields a field's resolution reads, i.e. its dependency edges in
/// the resolution DAG: `@env(.ref)` variable names, `@format` placeholders,
/// the match subject and its arms' references, and `@bind` sources. Paths into
/// a composed field depend on its head field only.
fn dependencies(field: &EntryField) -> Vec<&str> {
    fn head(p: &[String]) -> Option<&str> {
        p.first().map(String::as_str)
    }
    let mut deps: Vec<&str> = Vec::new();
    for source in &field.sources {
        if let Source::Env(EnvName::Field(fr)) = source {
            deps.extend(head(&fr.field));
        }
    }
    for part in field.format.iter().flatten() {
        if let TemplatePart::Field(p) = part {
            deps.extend(head(p));
        }
    }
    if let Some(select) = &field.select {
        deps.extend(head(&select.subject));
        for arm in &select.arms {
            if let ArmValue::Field(p) = &arm.value {
                deps.extend(head(p));
            }
        }
    }
    for Bind { source, .. } in &field.binds {
        deps.extend(head(source));
    }
    deps
}

/// Order fields so every dependency resolves before its dependents (Kahn over
/// the sibling-reference edges), keeping declaration order among ready fields.
/// The frontend already rejected cycles; if malformed input still has one, the
/// remaining fields append in declaration order rather than dropping.
fn resolution_order(fields: &[EntryField]) -> Vec<&EntryField> {
    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let mut placed: HashSet<&str> = HashSet::new();
    let mut out: Vec<&EntryField> = Vec::new();
    while out.len() < fields.len() {
        let mut progressed = false;
        for field in fields {
            if placed.contains(field.name.as_str()) {
                continue;
            }
            let ready = dependencies(field)
                .into_iter()
                .filter(|d| names.contains(d))
                .all(|d| placed.contains(d));
            if ready {
                placed.insert(field.name.as_str());
                out.push(field);
                progressed = true;
            }
        }
        if !progressed {
            for field in fields {
                if placed.insert(field.name.as_str()) {
                    out.push(field);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FieldRef, Select, SelectArm};
    use serde_json::json;

    fn field(name: &str, sources: Vec<Source>) -> EntryField {
        EntryField {
            name: name.into(),
            target: Tref::Prim(crate::ir::Prim::String),
            sources,
            format: None,
            transforms: vec![],
            select: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        }
    }

    fn entry_shape(id: &str, fields: Vec<EntryField>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Entry {
                fields,
                operations: vec![],
            },
            traits: vec![],
        }
    }

    fn module_of(shapes: Vec<Shape>) -> Module {
        Module {
            name: "m".into(),
            shapes,
            operations: vec![],
            extensions: vec![],
        }
    }

    #[test]
    fn fields_order_by_resolution_dependencies_keeping_declaration_order() {
        // endpoint selects on version and reads v1; v1 reads the variable named
        // by naming_field; naming_field formats from base.
        let mut endpoint = field("endpoint", vec![]);
        endpoint.select = Some(Select {
            subject: vec!["version".into()],
            arms: vec![
                SelectArm {
                    pattern: Some(json!("v1")),
                    value: ArmValue::Field(vec!["v1".into()]),
                },
                SelectArm {
                    pattern: None,
                    value: ArmValue::Lit(json!("fallback")),
                },
            ],
        });
        let mut v1 = field("v1", vec![]);
        v1.sources = vec![Source::Env(EnvName::Field(FieldRef {
            field: vec!["naming_field".into()],
        }))];
        let mut naming_field = field("naming_field", vec![]);
        naming_field.format = Some(vec![
            TemplatePart::Lit("EP_".into()),
            TemplatePart::Field(vec!["base".into()]),
        ]);
        let base = field("base", vec![Source::Arg]);
        let version = field(
            "version",
            vec![Source::Env(EnvName::Name("V".into())), Source::With],
        );

        let module = module_of(vec![entry_shape(
            "m#client",
            vec![endpoint, v1, naming_field, base, version],
        )]);
        let entries = module_entries(&module);
        let order: Vec<&str> = entries[0].fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            order,
            vec!["base", "version", "naming_field", "v1", "endpoint"]
        );
    }

    #[test]
    fn a_cycle_appends_the_rest_in_declaration_order_instead_of_dropping() {
        let mut a = field("a", vec![]);
        a.format = Some(vec![TemplatePart::Field(vec!["b".into()])]);
        let mut b = field("b", vec![]);
        b.format = Some(vec![TemplatePart::Field(vec!["a".into()])]);
        let module = module_of(vec![entry_shape("m#client", vec![a, b])]);
        let order: Vec<&str> = module_entries(&module)[0]
            .fields
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(order, vec!["a", "b"]);
    }

    #[test]
    fn args_and_withs_classify_by_source_in_declaration_order() {
        let module = module_of(vec![entry_shape(
            "m#client",
            vec![
                field("key", vec![Source::Arg]),
                field("name", vec![Source::With, Source::Default(json!("demo"))]),
                field("plain", vec![Source::Env(EnvName::Name("P".into()))]),
                field("second", vec![Source::Arg]),
            ],
        )]);
        let entries = module_entries(&module);
        let args: Vec<&str> = entries[0].args().iter().map(|f| f.name.as_str()).collect();
        let withs: Vec<&str> = entries[0].withs().iter().map(|f| f.name.as_str()).collect();
        assert_eq!(args, vec!["key", "second"]);
        assert_eq!(withs, vec!["name"]);
    }

    #[test]
    fn value_paths_cover_fields_and_their_composed_members() {
        let conf = Shape {
            id: "m#conf".into(),
            kind: ShapeKind::Config {
                fields: vec![field("api_key", vec![]), field("region", vec![])],
            },
            traits: vec![],
        };
        let mut settings = field("settings", vec![]);
        settings.target = Tref::Ref {
            id: "m#conf".into(),
            args: vec![],
        };
        let module = module_of(vec![
            conf,
            entry_shape(
                "m#client",
                vec![field("token", vec![Source::Arg]), settings],
            ),
        ]);
        let entries = module_entries(&module);
        let values = entries[0].value_paths(&module);
        let paths: Vec<&str> = values.iter().map(|v| v.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["token", "settings", "settings.api_key", "settings.region"]
        );
    }

    #[test]
    fn companion_names_prefix_only_in_the_multi_entry_case() {
        assert_eq!(companion_name("client", "settings", false), "settings");
        assert_eq!(
            companion_name("client", "settings", true),
            "client_settings"
        );
    }

    #[test]
    fn field_shapes_dispatch_on_the_referenced_shape_kind() {
        let conf = Shape {
            id: "m#conf".into(),
            kind: ShapeKind::Config { fields: vec![] },
            traits: vec![],
        };
        let creds = Shape {
            id: "m#creds".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits: vec![],
        };
        let mut composed = field("composed", vec![]);
        composed.target = Tref::Ref {
            id: "m#conf".into(),
            args: vec![],
        };
        let mut structured = field("structured", vec![]);
        structured.target = Tref::Ref {
            id: "m#creds".into(),
            args: vec![],
        };
        let mut labels = field("labels", vec![]);
        labels.target = Tref::Map(
            Box::new(Tref::Prim(crate::ir::Prim::String)),
            Box::new(Tref::Prim(crate::ir::Prim::String)),
        );
        let scalar = field("scalar", vec![]);
        let module = module_of(vec![
            conf,
            creds,
            entry_shape("m#client", vec![composed, structured, labels, scalar]),
        ]);
        let entries = module_entries(&module);
        let entry = &entries[0];
        let by_name = |n: &str| entry.fields.iter().find(|f| f.name == n).copied().unwrap();
        assert!(matches!(
            entry.field_shape(by_name("composed"), &module),
            FieldShape::Config(_)
        ));
        assert!(matches!(
            entry.field_shape(by_name("structured"), &module),
            FieldShape::Structured(_)
        ));
        assert!(matches!(
            entry.field_shape(by_name("labels"), &module),
            FieldShape::Json
        ));
        assert!(matches!(
            entry.field_shape(by_name("scalar"), &module),
            FieldShape::Scalar
        ));
    }

    #[test]
    fn guaranteed_follows_arg_default_and_derivation_inputs() {
        let mut derived = field("derived", vec![]);
        derived.format = Some(vec![TemplatePart::Field(vec!["base".into()])]);
        let mut floating = field("floating", vec![]);
        floating.format = Some(vec![TemplatePart::Field(vec!["maybe".into()])]);
        let module = module_of(vec![entry_shape(
            "m#client",
            vec![
                field("base", vec![Source::Arg]),
                field(
                    "with_default",
                    vec![Source::With, Source::Default(json!("d"))],
                ),
                field("maybe", vec![Source::Env(EnvName::Name("E".into()))]),
                derived,
                floating,
            ],
        )]);
        let entries = module_entries(&module);
        let entry = &entries[0];
        let by = |n: &str| entry.fields.iter().find(|f| f.name == n).copied().unwrap();
        assert!(entry.is_guaranteed(by("base")));
        assert!(entry.is_guaranteed(by("with_default")));
        assert!(!entry.is_guaranteed(by("maybe")));
        assert!(entry.is_guaranteed(by("derived")));
        assert!(!entry.is_guaranteed(by("floating")));
    }

    #[test]
    fn consumed_heads_read_the_raw_protocol_traits() {
        let op = Shape {
            id: "m#client.save".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![
                crate::ir::Trait {
                    id: "http".into(),
                    value: json!({
                        "method": "POST",
                        "path": "/v/{.tenant}/notes/{id}",
                        "endpoint": {"field": ["endpoint"]}
                    }),
                },
                crate::ir::Trait {
                    id: "header".into(),
                    value: json!(["X-Client", {"field": ["client_name"]}]),
                },
                crate::ir::Trait {
                    id: "timeout".into(),
                    value: json!([{"field": ["timeout"]}]),
                },
                crate::ir::Trait {
                    id: "retry".into(),
                    value: json!([{"field": ["max_retries"]}]),
                },
                crate::ir::Trait {
                    id: "wire_descriptor".into(),
                    value: json!({"opaque": true}),
                },
            ],
        };
        let shape = Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![],
                operations: vec![op],
            },
            traits: vec![],
        };
        let module = module_of(vec![shape]);
        let entries = module_entries(&module);
        assert_eq!(
            entries[0].consumed_field_heads(),
            vec![
                "tenant",
                "endpoint",
                "client_name",
                "timeout",
                "max_retries"
            ]
        );
    }

    #[test]
    fn has_entries_sees_only_entry_shapes() {
        assert!(!has_entries(&module_of(vec![])));
        assert!(has_entries(&module_of(vec![entry_shape("m#c", vec![])])));
    }
}

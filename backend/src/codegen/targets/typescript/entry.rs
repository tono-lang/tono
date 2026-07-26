//! The TypeScript entry client: the SDK construction surface an entry
//! declares, spelled as an exported class per entry (it replaces the generic
//! `HttpClient` for a module that declares one). The constructor takes the
//! `@arg` fields positionally and the `@with` fields as an optional config
//! object, resolves every declared source chain top-down, parses env values by
//! the field's type at the boundary (the error names the variable and the
//! type), lowers a match selection to a `switch`, decodes a structured source
//! strictly, composes a config through `@bind`, runs the bound `client_init`
//! hook over the resolved `Settings` (bespoke wins, by mutation), and
//! validates last. The resolved fields are frozen into `ClientOptions.values`,
//! so the descriptor's ref positions (endpoint, headers, timeout, retry)
//! resolve in the runtime without the client interpreting a descriptor.

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{doc_of, rename_of, type_ident_from_id};
use crate::codegen::entries::{
    companion_name, module_entries, op_local_name, EntryModel, FieldShape,
};
use crate::codegen::extensions::{bound_extensions, hook_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_descriptor};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::{type_expr_of, TsVal, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{
    ArmValue, EntryField, EnvName, Member, Module, Prim, Shape, ShapeKind, Source, TemplatePart,
    Tref,
};

/// The runtime package (same one the loose-op client uses).
use crate::codegen::targets::typescript::client::RUNTIME_PKG;

const BINDING_LANGS: [&str; 2] = ["ts", "typescript"];

fn pascal(name: &str) -> String {
    transform(
        name,
        SymbolKind::Type,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    )
}

fn camel(name: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Camel),
        None,
    )
}

struct Names {
    client: String,
    settings: String,
    config: String,
    op_prefix: String,
}

fn names(entry: &EntryModel<'_>, multi: bool) -> Names {
    Names {
        client: pascal(entry.name),
        settings: pascal(&companion_name(entry.name, "settings", multi)),
        config: pascal(&format!("{}_config", entry.name)),
        op_prefix: if multi {
            format!("{}_", entry.name)
        } else {
            String::new()
        },
    }
}

fn ts_type(t: &Tref) -> String {
    render_type(&type_expr_of(t), &TsRules)
}

fn field_camel(name: &str, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, None)
}

fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// The companion-file symbols a declared type drags into the serde file: the
/// branded well-known aliases and any named shape.
fn type_refs(t: &Tref, module: &Module) -> Vec<Symbol> {
    match t {
        Tref::Prim(Prim::Timestamp) => vec![module_symbol("Timestamp", module)],
        Tref::Prim(Prim::Date) => vec![module_symbol("LocalDate", module)],
        Tref::Prim(Prim::Duration) => vec![module_symbol("Duration", module)],
        Tref::Ref { id, .. } => {
            // A config interface lives in this same serde file (it is part of
            // the entry surface), so it needs no companion import.
            let is_config = module
                .shapes
                .iter()
                .any(|shape| shape.id == *id && matches!(shape.kind, ShapeKind::Config { .. }));
            if is_config {
                Vec::new()
            } else {
                vec![module_symbol(&type_ident_from_id(id), module)]
            }
        }
        Tref::List(inner) => type_refs(inner, module),
        Tref::Map(k, v) => {
            let mut out = type_refs(k, module);
            out.extend(type_refs(v, module));
            out
        }
        _ => Vec::new(),
    }
}

fn runtime_import(name: &str) -> Symbol {
    Symbol::imported(name, RUNTIME_PKG, name)
}

/// The zero value the mutable Settings draft starts from, per declared type.
fn zero_value(t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::Bool) => "false".into(),
        Tref::Prim(Prim::I64 | Prim::U64) => "0n".into(),
        Tref::Prim(
            Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32 | Prim::Float,
        ) => "0".into(),
        Tref::Prim(Prim::String | Prim::Uuid) => "\"\"".into(),
        Tref::Map(_, _) => "{}".into(),
        Tref::List(_) => "[]".into(),
        _ => format!("\"\" as unknown as {}", ts_type(t)),
    }
}

/// Whether the value is string-shaped in TypeScript (assignable from a raw env
/// string with at most a branded cast).
fn string_like(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(Prim::String | Prim::Uuid | Prim::Timestamp | Prim::Date | Prim::Duration)
            | Tref::Ref { .. }
    )
}

fn cast_string(t: &Tref, v: &str) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => v.to_string(),
        _ => format!("{v} as {}", ts_type(t)),
    }
}

/// A resolved field rendered as a string for template concatenation.
fn as_template_string(expr: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        _ => format!("String({expr})"),
    }
}

fn literal(t: &Tref, v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => cast_string(t, &format!("{s:?}")),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => match t {
            Tref::Prim(Prim::I64 | Prim::U64) => format!("{n}n"),
            _ => n.to_string(),
        },
        other => format!("{other}"),
    }
}

fn pattern_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        other => format!("{other}"),
    }
}

#[derive(Default)]
struct Helpers {
    read_env: bool,
    duration_ms: bool,
    transforms: BTreeSet<&'static str>,
}

/// Every serde-file declaration of the module's entry surface: the config and
/// Settings interfaces, the hook wrappers (when this module has no loose-op
/// client already emitting them), and per entry the descriptor constants, the
/// class, and its methods.
pub fn entry_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let entries = module_entries(module);
    if entries.is_empty() {
        return Vec::new();
    }
    let multi = entries.len() > 1;
    let bound = bound_extensions(module, &BINDING_LANGS);
    let mut helpers = Helpers::default();
    let mut decls = Vec::new();
    decls.extend(config_interfaces(module, config));
    // The transport wrappers are shared spellings with the loose-op client;
    // emit them here only when that client is absent, so a mixed module does
    // not declare them twice.
    let loose_client_present = module
        .operations
        .iter()
        .any(|op| wire_descriptor(op).is_some());
    if !loose_client_present {
        decls.extend(transport_hook_wrappers(&bound, module));
    }
    decls.extend(client_init_wrapper(&bound, &entries, multi, module));
    for entry in &entries {
        let n = names(entry, multi);
        decls.push(settings_interface(entry, &n, config, module));
        decls.extend(config_object_interface(entry, &n, config, module));
        decls.extend(descriptor_decls(entry, &n));
        decls.push(class_decl(
            entry,
            &n,
            module,
            config,
            &bound,
            &mut helpers,
            multi,
        ));
        decls.extend(discriminator_decls_for(entry, &n, module));
    }
    decls.extend(helper_decls(&helpers));
    decls
}

/// The construction-only config interfaces (they never cross the wire, so the
/// regular type emission skips them).
fn config_interfaces(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    module
        .shapes
        .iter()
        .filter_map(|shape| {
            let ShapeKind::Config { fields } = &shape.kind else {
                return None;
            };
            let name = type_ident_from_id(&shape.id);
            let members: String = fields
                .iter()
                .map(|f| {
                    format!(
                        "  {}: {};\n",
                        field_camel(&f.name, config),
                        ts_type(&f.target)
                    )
                })
                .collect();
            Some(Decl::raw(format!(
                "// {name} is a construction-only composition of the entry surface; it\n\
                 // never crosses the wire.\n\
                 export interface {name} {{\n{members}}}"
            )))
        })
        .collect()
}

/// The Settings interface: every resolved field plus the transport slots the
/// bespoke `client_init` hook may fill.
fn settings_interface(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    entry_module: &Module,
) -> Decl {
    let mut fields = String::new();
    let mut refs = vec![runtime_import("CanonicalTransport")];
    for f in entry.declared() {
        fields.push_str(&format!(
            "  {}: {};\n",
            field_camel(&f.name, config),
            ts_type(&f.target)
        ));
        refs.extend(type_refs(&f.target, entry_module));
    }
    Decl::raw_with(
        format!(
            "// {settings} are the resolved construction values of the {entry} entry,\n\
             // handed to the client_init hook before validation: bespoke code may\n\
             // overwrite any field (bespoke wins) and set transport through the slots.\n\
             // Exactly one transport slot may be set: fetch (native) or transport\n\
             // (canonical). headers are the base request headers (bespoke auth writes\n\
             // here); a declared @header wins only where nothing else set the name.\n\
             export interface {settings} {{\n{fields}  fetch?: typeof fetch;\n  transport?: CanonicalTransport;\n  headers: Record<string, string>;\n}}",
            settings = n.settings,
            entry = entry.name,
        ),
        refs,
    )
}

/// The optional config object: one optional property per `@with` field.
fn config_object_interface(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    entry_module: &Module,
) -> Vec<Decl> {
    let withs = entry.withs();
    if withs.is_empty() {
        return Vec::new();
    }
    let fields: String = withs
        .iter()
        .map(|f| {
            format!(
                "  {}?: {};\n",
                field_camel(&f.name, config),
                ts_type(&f.target)
            )
        })
        .collect();
    let refs = withs
        .iter()
        .flat_map(|f| type_refs(&f.target, entry_module))
        .collect();
    vec![Decl::raw_with(
        format!(
            "// {config} carries the optional (@with) construction values of {client}.\n\
             export interface {config} {{\n{fields}}}",
            config = n.config,
            client = n.client,
        ),
        refs,
    )]
}

fn descriptor_var(n: &Names, op: &Shape) -> String {
    camel(&format!(
        "{}{}_descriptor",
        n.op_prefix,
        op_local_name(&op.id)
    ))
}

fn discriminator_name(n: &Names, op: &Shape) -> String {
    format!(
        "decode{}Error",
        pascal(&format!("{}{}", n.op_prefix, op_local_name(&op.id)))
    )
}

fn method_name(op: &Shape, config: &CasingConfig) -> String {
    let rename = rename_of(&op.traits, LANG);
    transform(
        op_local_name(&op.id),
        SymbolKind::Method,
        config,
        rename.as_deref(),
    )
}

/// The JSON descriptor embedded as a JavaScript string literal the runtime
/// parses at load (encode twice: an opaque blob, no field ever read).
fn embed(descriptor: &serde_json::Value) -> String {
    let json = serde_json::to_string(descriptor).unwrap_or_else(|_| "null".into());
    serde_json::to_string(&json).unwrap_or_else(|_| "\"null\"".into())
}

fn descriptor_decls(entry: &EntryModel<'_>, n: &Names) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter_map(|op| {
            let descriptor = wire_descriptor(op)?;
            Some(Decl::raw_with(
                format!(
                    "const {var}: WireDescriptor = JSON.parse({literal});",
                    var = descriptor_var(n, op),
                    literal = embed(descriptor),
                ),
                vec![runtime_import("WireDescriptor")],
            ))
        })
        .collect()
}

/// The discrimination functions for the entry's operations, named through the
/// entry rule.
fn discriminator_decls_for(entry: &EntryModel<'_>, n: &Names, module: &Module) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            super::errors::discriminator_fn_named(&discriminator_name(n, op), &ordered, module)
        })
        .collect()
}

/// The runtime-facing hook wrappers (`before_request`/`after_response`),
/// shared spelling with the loose-op client.
fn transport_hook_wrappers(bound: &[BoundExtension<'_>], module: &Module) -> Vec<Decl> {
    let en = error_names();
    let mut decls = Vec::new();
    for (slot, ty, wrapper) in [
        ("before_request", "CanonicalRequest", "wrapBeforeRequest"),
        ("after_response", "CanonicalResponse", "wrapAfterResponse"),
    ] {
        let Some(b) = hook_binding(bound, slot) else {
            continue;
        };
        decls.push(Decl::raw_with(
            format!(
                "async function {wrapper}(x: {ty}): Promise<{ty}> {{\n  try {{\n    return await {sym}(x);\n  }} catch (e) {{\n    // A declared SDK error passes through typed; anything else is bespoke\n    // failure surfaced as a ContractError.\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"{slot}\", e);\n  }}\n}}",
                sym = b.symbol,
                root = en.root,
                contract = en.contract,
            ),
            vec![
                Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
                runtime_import(ty),
                module_symbol(&en.root, module),
                module_symbol(&en.contract, module),
            ],
        ));
    }
    if let Some(b) = hook_binding(bound, "on_error") {
        decls.push(Decl::raw_with(
            format!(
                "function wrapOnError(err: {root}): {root} {{\n  try {{\n    return {sym}(err);\n  }} catch (e) {{\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"on_error\", e);\n  }}\n}}",
                root = en.root,
                contract = en.contract,
                sym = b.symbol,
            ),
            vec![
                Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
                module_symbol(&en.root, module),
                module_symbol(&en.contract, module),
            ],
        ));
    }
    decls
}

/// The `client_init` boundary wrapper for entries: the bespoke symbol mutates
/// the resolved Settings in place (single-entry modules only: the bespoke
/// symbol has one signature).
fn client_init_wrapper(
    bound: &[BoundExtension<'_>],
    entries: &[EntryModel<'_>],
    multi: bool,
    module: &Module,
) -> Vec<Decl> {
    let Some(b) = hook_binding(bound, "client_init") else {
        return Vec::new();
    };
    if multi {
        return Vec::new();
    }
    let Some(entry) = entries.first() else {
        return Vec::new();
    };
    let en = error_names();
    let n = names(entry, multi);
    vec![Decl::raw_with(
        format!(
            "// The client_init bridge: bespoke code runs over the resolved Settings\n\
             // (bespoke wins) before validation.\n\
             function wrapClientInit(settings: {settings}): void {{\n  try {{\n    {sym}(settings);\n  }} catch (e) {{\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"client_init\", e);\n  }}\n}}",
            settings = n.settings,
            sym = b.symbol,
            root = en.root,
            contract = en.contract,
        ),
        vec![
            Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
            module_symbol(&en.root, module),
            module_symbol(&en.contract, module),
        ],
    )]
}

/// A bound file path as a TypeScript import specifier (mirrors the loose-op
/// client's rule).
fn import_specifier(module: &str) -> String {
    let path = module
        .strip_suffix(".ts")
        .or_else(|| module.strip_suffix(".tsx"))
        .unwrap_or(module);
    if path.starts_with('.') || path.starts_with('/') {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

fn helper_decls(helpers: &Helpers) -> Vec<Decl> {
    let mut decls = Vec::new();
    if helpers.read_env {
        decls.push(Decl::raw(
            "// readEnv treats an unset and an empty variable the same: empty means\n\
             // not set, per the declared-source contract.\n\
             function readEnv(name: string): string | undefined {\n\
             \x20 const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;\n\
             \x20 const v = env?.[name];\n\
             \x20 return v === undefined || v === \"\" ? undefined : v;\n\
             }"
            .to_string(),
        ));
    }
    if helpers.duration_ms {
        decls.push(Decl::raw(
            "// durationToMs parses the duration spelling shared across targets\n\
             // (Go's ParseDuration grammar: optional sign, bare zero, unit runs)\n\
             // into the runtime's millisecond values.\n\
             function durationToMs(v: string): number {\n\
             \x20 let rest = v;\n\
             \x20 let sign = 1;\n\
             \x20 if (rest.startsWith(\"-\")) {\n\
             \x20   sign = -1;\n\
             \x20   rest = rest.slice(1);\n\
             \x20 } else if (rest.startsWith(\"+\")) {\n\
             \x20   rest = rest.slice(1);\n\
             \x20 }\n\
             \x20 if (rest === \"0\") {\n\
             \x20   return 0;\n\
             \x20 }\n\
             \x20 const re = /(\\d+(?:\\.\\d+)?)(ns|us|\\u00b5s|ms|s|m|h)/gy;\n\
             \x20 const unit: Record<string, number> = { ns: 1e-6, us: 1e-3, \"\\u00b5s\": 1e-3, ms: 1, s: 1000, m: 60000, h: 3600000 };\n\
             \x20 let total = 0;\n\
             \x20 let consumed = 0;\n\
             \x20 for (let m = re.exec(rest); m !== null; m = re.exec(rest)) {\n\
             \x20   total += Number(m[1]) * unit[m[2]];\n\
             \x20   consumed = re.lastIndex;\n\
             \x20 }\n\
             \x20 if (consumed !== rest.length || rest.length === 0) {\n\
             \x20   throw new Error(`invalid duration ${JSON.stringify(v)}`);\n\
             \x20 }\n\
             \x20 return sign * total;\n\
             }"
            .to_string(),
        ));
    }
    if !helpers.transforms.is_empty() {
        decls.push(Decl::raw(
            "// strTransformWords splits a resolved value for the casing transforms:\n\
             // runs of spaces, hyphens, and underscores separate words.\n\
             function strTransformWords(s: string): string[] {\n\
             \x20 return s.split(/[ _-]+/).filter((w) => w !== \"\");\n\
             }"
            .to_string(),
        ));
        for t in &helpers.transforms {
            let (name, body) = match *t {
                "upper_snake" => (
                    "strUpperSnake",
                    "  return strTransformWords(s).map((w) => w.toUpperCase()).join(\"_\");",
                ),
                "snake" => (
                    "strSnake",
                    "  return strTransformWords(s).map((w) => w.toLowerCase()).join(\"_\");",
                ),
                "kebab" => (
                    "strKebab",
                    "  return strTransformWords(s).map((w) => w.toLowerCase()).join(\"-\");",
                ),
                "pascal" => (
                    "strPascal",
                    "  return strTransformWords(s).map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase()).join(\"\");",
                ),
                _ => continue,
            };
            decls.push(Decl::raw(format!(
                "function {name}(s: string): string {{\n{body}\n}}"
            )));
        }
    }
    decls
}

fn apply_transforms(expr: String, transforms: &[String], helpers: &mut Helpers) -> String {
    let mut out = expr;
    for t in transforms {
        out = match t.as_str() {
            "trim" => format!("({out}).trim()"),
            "lower" => format!("({out}).toLowerCase()"),
            "upper" => format!("({out}).toUpperCase()"),
            "upper_snake" | "snake" | "kebab" | "pascal" => {
                helpers.transforms.insert(match t.as_str() {
                    "upper_snake" => "upper_snake",
                    "snake" => "snake",
                    "kebab" => "kebab",
                    _ => "pascal",
                });
                format!("str{}({out})", pascal(t))
            }
            _ => out,
        };
    }
    out
}

/// The class per entry: constructor (resolution, bridge, validation, frozen
/// options) plus one async method per operation.
fn class_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
) -> Decl {
    let en = error_names();
    let mut refs = vec![
        runtime_import("ClientOptions"),
        runtime_import("execute"),
        module_symbol(&en.transport, module),
    ];
    for f in &entry.fields {
        refs.extend(type_refs(&f.target, module));
        if let FieldShape::Structured(shape) = entry.field_shape(f, module) {
            if validation::shape_has_checks(shape) {
                refs.push(module_symbol(
                    &format!("validate{}", type_ident_from_id(&shape.id)),
                    module,
                ));
                refs.push(module_symbol(&en.validation, module));
            }
        }
    }
    let mut body = String::new();

    let args = entry.args();
    let mut params: Vec<String> = args
        .iter()
        .map(|f| format!("{}: {}", camel(&f.name), ts_type(&f.target)))
        .collect();
    if !entry.withs().is_empty() {
        params.push(format!("config: {} = {{}}", n.config));
    }

    // The mutable Settings draft, zeroed, then resolved in dependency order.
    body.push_str(&format!(
        "    const s: {settings} = {{ {zeros}headers: {{}} }};\n",
        settings = n.settings,
        zeros = entry
            .declared()
            .iter()
            .map(|f| format!(
                "{}: {}, ",
                field_camel(&f.name, config),
                zero_value(&f.target)
            ))
            .collect::<String>(),
    ));

    let mut r = Resolver {
        entry,
        module,
        config,
        helpers,
        body: &mut body,
    };
    for field in &entry.fields {
        r.emit_field(field);
    }

    if hook_binding(bound, "client_init").is_some() && !multi {
        body.push_str("    wrapClientInit(s);\n");
    }

    // Consumed chains must hold a value once construction finishes.
    for head in entry.consumed_field_heads() {
        let Some(field) = entry.fields.iter().find(|f| f.name == head) else {
            continue;
        };
        if entry.is_guaranteed(field) || !string_like(&field.target) {
            continue;
        }
        if !matches!(entry.field_shape(field, module), FieldShape::Scalar) {
            continue;
        }
        body.push_str(&format!(
            "    if (s.{ident} === {zero}) {{\n      throw new Error(\"{name} <- \" + ({why} || \"no value\"));\n    }}\n",
            ident = field_camel(&head, config),
            zero = cast_string(&field.target, "\"\""),
            why = why_var(&head),
            name = head,
        ));
    }

    // Declared validation runs last, over what bespoke left in place.
    let mut guards = String::new();
    for field in &entry.fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = Member {
            name: field.name.clone(),
            target: field.target.clone(),
            required: true,
            default: None,
            constraints: field.constraints.clone(),
            traits: field.traits.clone(),
        };
        let composed = matches!(entry.field_shape(field, module), FieldShape::Config(_));
        for line in validation::guard_lines(&[member], &TsVal, "s.", config, LANG) {
            let guard = if entry.is_guaranteed(field) || composed {
                line.condition.clone()
            } else {
                format!("{} === \"\" && {}", why_var(&field.name), line.condition)
            };
            guards.push_str(&format!(
                "    if ({guard}) {{\n      violations.push({{ field: {field:?}, constraint: {constraint:?}, message: {message:?} }});\n    }}\n",
                field = line.field,
                constraint = line.constraint,
                message = line.message,
            ));
        }
    }
    if !guards.is_empty() {
        refs.push(module_symbol(&en.violation, module));
        refs.push(module_symbol(&en.validation, module));
        body.push_str(&format!(
            "    const violations: {violation}[] = [];\n{guards}    if (violations.length > 0) {{\n      throw new {validation}(violations);\n    }}\n",
            violation = en.violation,
            validation = en.validation,
        ));
    }

    // Freeze the resolved values for the runtime's ref positions.
    body.push_str("    const values: Record<string, unknown> = {};\n");
    for vp in entry.value_paths(module) {
        let Some(_expr) = value_expr(&vp, config) else {
            continue;
        };
        let composed = matches!(entry.field_shape(vp.field, module), FieldShape::Config(_));
        let assign = if let Tref::Prim(Prim::Duration) = vp.target {
            helpers.duration_ms = true;
            format!(
                "    try {{\n      values[{path:?}] = durationToMs(String(s.{f}));\n    }} catch {{\n      throw new Error(`{path}: invalid duration ${{JSON.stringify(String(s.{f}))}}`);\n    }}\n",
                path = vp.path,
                f = access(&vp, config),
            )
        } else {
            format!(
                "    values[{path:?}] = {value};\n",
                path = vp.path,
                value = value_cast(vp.target, &format!("s.{}", access(&vp, config))),
            )
        };
        match presence_guard(entry, &vp).filter(|_| !composed) {
            Some(guard) => body.push_str(&format!("    if ({guard}) {{\n  {assign}    }}\n",)),
            None => body.push_str(&assign),
        }
    }

    // The frozen client options: entry construction leaves baseUrl empty (the
    // per-operation endpoint resolves from the descriptor's ref), and the
    // transport slots come off the bridged Settings.
    body.push_str(
        "    this.settings = s;\n    this.options = { baseUrl: \"\", fetch: s.fetch, transport: s.transport, headers: s.headers, values };\n",
    );

    let hooks_field = {
        let mut slots = Vec::new();
        if hook_binding(bound, "before_request").is_some() {
            slots.push("before_request: wrapBeforeRequest".to_string());
        }
        if hook_binding(bound, "after_response").is_some() {
            slots.push("after_response: wrapAfterResponse".to_string());
        }
        if slots.is_empty() {
            String::new()
        } else {
            refs.push(runtime_import("Hooks"));
            format!(
                "  private readonly hooks: Hooks = {{ {} }};\n",
                slots.join(", ")
            )
        }
    };
    let passes_hooks = !hooks_field.is_empty();

    let mut methods = String::new();
    for op in entry.operations {
        methods.push_str(&op_method(
            entry,
            n,
            op,
            module,
            config,
            bound,
            passes_hooks,
            &mut refs,
        ));
        methods.push('\n');
    }

    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let text = format!(
        "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
         // constructor takes the @arg values positionally and the @with values as a\n\
         // config object; construction resolves the declared sources, runs the\n\
         // client_init bridge, validates, and freezes the runtime options.\n\
         export class {client} {{\n\
         \x20 readonly settings: {settings};\n\
         \x20 private readonly options: ClientOptions;\n\
         {hooks_field}\
         \x20 constructor({params}) {{\n{body}  }}\n\n{methods}}}",
        client = n.client,
        entry_name = entry.name,
        settings = n.settings,
        params = params.join(", "),
    );
    Decl::raw_with(text, refs)
}

/// One operation method (mirrors the loose-op client's outcome mapping).
#[allow(clippy::too_many_arguments)]
fn op_method(
    entry: &EntryModel<'_>,
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    passes_hooks: bool,
    refs: &mut Vec<Symbol>,
) -> String {
    let _ = entry;
    let en = error_names();
    let name = method_name(op, config);
    let (input, output) = crate::codegen::ops::op_io(op);
    let has_on_error = hook_binding(bound, "on_error").is_some();
    let throw = |expr: String| {
        if has_on_error {
            format!("throw wrapOnError({expr});")
        } else {
            format!("throw {expr};")
        }
    };

    if wire_descriptor(op).is_none() {
        // A bespoke-bound operation: invoking the bound impl through the
        // generated glue is not wired yet, so the method reports that plainly
        // while keeping the declared signature.
        refs.push(module_symbol(&en.contract, module));
        let param = match input {
            Some(t) => {
                refs.extend(type_refs(t, module));
                format!("input: {}", render_type(&type_expr_of(t), &TsRules))
            }
            None => String::new(),
        };
        if let Some(t) = output {
            refs.extend(type_refs(t, module));
        }
        let ret = output
            .map(|t| render_type(&type_expr_of(t), &TsRules))
            .unwrap_or_else(|| "void".to_string());
        return format!(
            "  async {name}({param}): Promise<{ret}> {{\n    {t}\n  }}",
            t = throw(format!(
                "new {}({:?}, new Error(\"operation has no transport binding\"))",
                en.contract,
                op_local_name(&op.id),
            )),
        );
    }

    let (param, input_expr) = match input {
        Some(t) => {
            refs.extend(type_refs(t, module));
            let ty = render_type(&type_expr_of(t), &TsRules);
            let expr = match t {
                Tref::Ref { id, .. } => {
                    format!("encode{}(input)", type_ident_from_id(id))
                }
                _ => "input".to_string(),
            };
            (format!("input: {ty}"), expr)
        }
        None => (String::new(), "{}".to_string()),
    };
    if let Some(t) = output {
        refs.extend(type_refs(t, module));
    }
    let ret = output
        .map(|t| render_type(&type_expr_of(t), &TsRules))
        .unwrap_or_else(|| "void".to_string());

    let error_line = if declared_errors(op, module).is_empty() {
        refs.push(module_symbol(&en.api, module));
        throw(format!("new {}(outcome.status, outcome.body)", en.api))
    } else {
        throw(format!(
            "{}(outcome.status, outcome.body)",
            discriminator_name(n, op)
        ))
    };
    let success_block = match output {
        Some(Tref::Ref { id, .. }) => {
            refs.push(module_symbol(&en.decode, module));
            let out_name = type_ident_from_id(id);
            format!(
                "    try {{\n      return decode{out_name}(JSON.parse(outcome.body));\n    }} catch {{\n      {t}\n    }}",
                t = throw(format!(
                    "new {}(\"$\", \"{out_name}\", outcome.body)",
                    en.decode
                )),
            )
        }
        Some(_) => format!("    return JSON.parse(outcome.body) as {ret};"),
        None => "    return;".to_string(),
    };
    let hooks_arg = if passes_hooks { ", this.hooks" } else { "" };
    let transport_throw = throw(format!("new {}(outcome.cause)", en.transport));
    let doc = doc_of(&op.traits)
        .map(|d| format!("  // {}\n", d.replace('\n', "\n  // ")))
        .unwrap_or_default();
    format!(
        "{doc}  async {name}({param}): Promise<{ret}> {{\n\
         \x20   const outcome = await execute({descriptor}, {input_expr}, this.options{hooks_arg});\n\
         \x20   if (outcome.outcome === \"transport\") {{\n\
         \x20     {transport_throw}\n\
         \x20   }}\n\
         \x20   if (outcome.outcome === \"error\") {{\n\
         \x20     {error_line}\n\
         \x20   }}\n\
         {success_block}\n\
         \x20 }}",
        descriptor = descriptor_var(n, op),
    )
}

fn why_var(field: &str) -> String {
    camel(&format!("{field}_why"))
}

fn access(vp: &crate::codegen::entries::ValuePath<'_>, config: &CasingConfig) -> String {
    match &vp.member {
        None => field_camel(&vp.field.name, config),
        Some(member) => format!(
            "{}.{}",
            field_camel(&vp.field.name, config),
            field_camel(member, config)
        ),
    }
}

fn value_expr(
    vp: &crate::codegen::entries::ValuePath<'_>,
    config: &CasingConfig,
) -> Option<String> {
    if matches!(
        vp.target,
        Tref::Ref { .. } | Tref::Map(_, _) | Tref::List(_)
    ) {
        return None;
    }
    Some(format!("s.{}", access(vp, config)))
}

/// The conversion into the runtime's value positions: bigints narrow to
/// numbers (the descriptor's numeric refs), everything else passes as is.
fn value_cast(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(Prim::I64 | Prim::U64) => format!("Number({expr})"),
        _ => expr.to_string(),
    }
}

fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
) -> Option<String> {
    if entry.is_guaranteed(vp.field) {
        return None;
    }
    Some(format!("{} === \"\"", why_var(&vp.field.name)))
}

/// The per-field resolution emitter, mirroring the Go one statement for
/// statement so both SDKs construct identically.
struct Resolver<'a, 'b> {
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    config: &'a CasingConfig,
    helpers: &'b mut Helpers,
    body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn push(&mut self, s: &str) {
        self.body.push_str(s);
    }

    fn ident(&self, name: &str) -> String {
        format!("s.{}", field_camel(name, self.config))
    }

    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for seg in path {
            out.push('.');
            out.push_str(&field_camel(seg, self.config));
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        let head = self
            .entry
            .fields
            .iter()
            .find(|f| path.first().is_some_and(|h| *h == f.name));
        let Some(head) = head else {
            return Tref::Prim(Prim::String);
        };
        if path.len() == 1 {
            return head.target.clone();
        }
        if let Tref::Ref { id, .. } = &head.target {
            if let Some(shape) = self.module.shapes.iter().find(|s| s.id == *id) {
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
        Tref::Prim(Prim::String)
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry
            .fields
            .iter()
            .find(|f| f.name == name)
            .is_some_and(|f| self.entry.is_guaranteed(f))
    }

    fn emit_field(&mut self, field: &EntryField) {
        match self.entry.field_shape(field, self.module) {
            FieldShape::Config(shape) => self.emit_config(field, shape),
            FieldShape::Structured(shape) => self.emit_structured(field, shape),
            FieldShape::Json => self.emit_json(field),
            FieldShape::Scalar => self.emit_scalar(field),
        }
    }

    fn emit_scalar(&mut self, field: &EntryField) {
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!(
                "    {} = {};\n",
                self.ident(&field.name),
                camel(&field.name)
            );
            self.push(&assign);
            return;
        }
        if field.select.is_some() {
            self.emit_select(field);
            return;
        }
        if field.format.is_some() {
            self.emit_format(field);
            return;
        }
        self.emit_chain(field);
    }

    fn emit_chain(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if self.entry.is_guaranteed(field) {
            let stmts = self.chain_cascade(field, &dest);
            self.push(&stmts);
        } else {
            let why = why_var(&field.name);
            self.push(&format!("    let {why} = \"no source\";\n"));
            let stmts = self.chain_sequential(field, &dest, &why);
            self.push(&stmts);
        }
    }

    fn with_access(&self, field: &EntryField) -> String {
        format!("config.{}", field_camel(&field.name, self.config))
    }

    /// A guaranteed chain, spelled with a set-flag so every env variable is
    /// read exactly once and the `@default` closes the chain.
    fn chain_cascade(&mut self, field: &EntryField, dest: &str) -> String {
        // A chain that is just the default needs no flag.
        if let [Source::Default(v)] = field.sources.as_slice() {
            return format!("    {dest} = {};\n", literal(&field.target, v));
        }
        let flag = camel(&format!("{}_set", field.name));
        let mut out = format!("    let {flag} = false;\n");
        for source in &field.sources {
            match source {
                Source::With => {
                    let acc = self.with_access(field);
                    out.push_str(&format!(
                        "    if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {flag} = true;\n    }}\n"
                    ));
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    out.push_str(&format!(
                        "    if (!{flag}) {{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}        {flag} = true;\n      }}\n    }}\n",
                        parse = self
                            .env_parse(field, dest, &self.env_label(name))
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                    ));
                }
                Source::Default(v) => {
                    out.push_str(&format!(
                        "    if (!{flag}) {{\n      {dest} = {};\n    }}\n",
                        literal(&field.target, v),
                    ));
                    return out;
                }
                Source::Arg => {}
            }
        }
        out
    }

    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => {
                    let acc = self.with_access(field);
                    format!(
                        "if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {why} = \"\";\n    }} else {{\n      {why} = \"not configured\";\n    }}\n"
                    )
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    format!(
                        "{pre}{{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}        {why} = \"\";\n      }} else {{\n        {why} = {miss};\n      }}\n    }}{post}\n",
                        parse = self
                            .env_parse(field, dest, &label)
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                        post = "",
                    )
                }
                Source::Default(v) => format!(
                    "{dest} = {lit};\n    {why} = \"\";\n",
                    lit = literal(&field.target, v),
                ),
                Source::Arg => continue,
            };
            if first {
                out.push_str(&format!("    {step}"));
                first = false;
            } else {
                out.push_str(&format!("    if ({why} !== \"\") {{\n    {step}    }}\n",));
            }
        }
        out
    }

    fn env_name_prereq(&self, name: &EnvName, why: &str) -> String {
        let EnvName::Field(fr) = name else {
            return String::new();
        };
        let Some(head) = fr.field.first() else {
            return String::new();
        };
        if self.guaranteed(head) {
            return String::new();
        }
        format!(
            "if ({head_why} !== \"\") {{\n      {why} = \"{head} <- \" + {head_why};\n    }} else ",
            head_why = why_var(head),
        )
    }

    fn env_lookup(&mut self, name: &EnvName) -> String {
        self.helpers.read_env = true;
        match name {
            EnvName::Name(n) => format!("readEnv({n:?})"),
            EnvName::Field(fr) => {
                let expr = self.path_expr(&fr.field);
                let t = self.path_type(&fr.field);
                format!("readEnv({})", as_template_string(&expr, &t))
            }
        }
    }

    fn env_label(&self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{n:?}"),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                as_template_string(&self.path_expr(&fr.field), &t)
            }
        }
    }

    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                format!(
                    "\"env \" + {} + \": empty\"",
                    as_template_string(&self.path_expr(&fr.field), &t)
                )
            }
        }
    }

    /// Parse a raw env string `v` into the destination, by the declared type;
    /// a parse failure fails construction naming the variable and the type.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => format!(
                "      if (v === \"true\" || v === \"1\") {{\n        {dest} = true;\n      }} else if (v === \"false\" || v === \"0\") {{\n        {dest} = false;\n      }} else {{\n        throw new Error(`${{{label}}}: invalid bool ${{JSON.stringify(v)}} (want true/false/1/0)`);\n      }}\n"
            ),
            Tref::Prim(
                p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32),
            ) => format!(
                "      {{\n        const n = Number(v);\n        if (!Number.isInteger(n)) {{\n          throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n        }}\n        {dest} = n;\n      }}\n",
                prim = prim_name(p),
            ),
            Tref::Prim(p @ (Prim::I64 | Prim::U64)) => format!(
                "      try {{\n        {dest} = BigInt(v);\n      }} catch {{\n        throw new Error(`${{{label}}}: invalid {prim} ${{JSON.stringify(v)}}`);\n      }}\n",
                prim = prim_name(p),
            ),
            Tref::Prim(Prim::Float) => format!(
                "      {{\n        const n = Number(v);\n        if (!Number.isFinite(n)) {{\n          throw new Error(`${{{label}}}: invalid float ${{JSON.stringify(v)}}`);\n        }}\n        {dest} = n;\n      }}\n"
            ),
            Tref::Prim(Prim::Duration) => {
                self.helpers.duration_ms = true;
                format!(
                    "      try {{\n        durationToMs(v);\n      }} catch {{\n        throw new Error(`${{{label}}}: invalid duration ${{JSON.stringify(v)}}`);\n      }}\n      {dest} = v as Duration;\n"
                )
            }
            _ => format!("      {dest} = {};\n", cast_string(t, "v")),
        }
    }

    fn emit_select(&mut self, field: &EntryField) {
        let Some(select) = field.select.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let guaranteed = self.entry.is_guaranteed(field);
        let why = why_var(&field.name);
        if !guaranteed {
            self.push(&format!("    let {why} = \"\";\n"));
        }
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        let mut saw_wildcard = false;
        for arm in &select.arms {
            let stmts = self.arm_stmts(field, &arm.value, &dest, &why, guaranteed);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "      case {}: {{\n{stmts}        break;\n      }}\n",
                    pattern_literal(p),
                )),
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!(
                        "      default: {{\n{stmts}        break;\n      }}\n"
                    ));
                }
            }
        }
        if !saw_wildcard {
            let miss = if guaranteed {
                String::new()
            } else {
                format!("        {why} = \"match: unmatched value\";\n")
            };
            arms.push_str(&format!(
                "      default: {{\n{miss}        break;\n      }}\n"
            ));
        }
        let switch = format!("    switch ({subject_expr}) {{\n{arms}    }}\n");
        if !self.guaranteed(&subject_head) {
            self.push(&format!(
                "    if ({subj_why} !== \"\") {{\n      {why} = \"{subject_head} <- \" + {subj_why};\n    }} else {{\n  {switch}    }}\n",
                subj_why = why_var(&subject_head),
            ));
        } else {
            self.push(&switch);
        }
    }

    fn arm_stmts(
        &mut self,
        field: &EntryField,
        value: &ArmValue,
        dest: &str,
        why: &str,
        guaranteed: bool,
    ) -> String {
        match value {
            ArmValue::Lit(v) => format!("        {dest} = {};\n", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("        {dest} = {expr};\n")
                } else {
                    format!(
                        "        if ({head_why} !== \"\") {{\n          {why} = \"{head} <- \" + {head_why};\n        }} else {{\n          {dest} = {expr};\n        }}\n",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let stub = EntryField {
                    name: field.name.clone(),
                    target: field.target.clone(),
                    sources: sources.clone(),
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                };
                let inner = if guaranteed {
                    self.chain_cascade(&stub, dest)
                } else {
                    format!(
                        "    {why} = \"no source\";\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                };
                inner
                    .lines()
                    .map(|l| format!("    {l}\n"))
                    .collect::<String>()
            }
        }
    }

    fn emit_format(&mut self, field: &EntryField) {
        let Some(format_parts) = field.format.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let mut concat: Vec<String> = Vec::new();
        let mut absent_deps: Vec<String> = Vec::new();
        for part in &format_parts {
            match part {
                TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
                TemplatePart::Field(p) => {
                    let head = p.first().cloned().unwrap_or_default();
                    if !self.guaranteed(&head) && !absent_deps.contains(&head) {
                        absent_deps.push(head.clone());
                    }
                    let t = self.path_type(p);
                    let expr = self.path_expr(p);
                    concat.push(as_template_string(&expr, &t));
                }
                TemplatePart::Input(_) => concat.push("\"\"".to_string()),
            }
        }
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("    {dest} = {};\n", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        let mut out = format!("    let {why} = \"\";\n");
        for (i, dep) in absent_deps.iter().enumerate() {
            out.push_str(&format!(
                "{}if ({dep_why} !== \"\") {{\n      {why} = \"{dep} <- \" + {dep_why};\n    }}",
                if i == 0 { "    " } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        out.push_str(&format!(" else {{\n  {assign}    }}\n"));
        self.push(&out);
    }

    /// A structured source: JSON in an env variable decoded strictly (unknown
    /// fields rejected, required members checked by name), validated at
    /// construction. The error carries the variable's name as context.
    /// A structured source: an explicit `@arg`/`@with` value passes typed, a
    /// JSON env value decodes strictly (required members first, then unknown
    /// fields, then per-member scalar type checks, mirroring the Go order and
    /// strictness), and declared validation runs at construction.
    fn emit_structured(&mut self, field: &EntryField, shape: &Shape) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("    {dest} = {};\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        // Without @arg a structured field can never be guaranteed (@default
        // does not apply to it), so the chain is always why-tracked.
        let why = why_var(&field.name);
        self.push(&format!("    let {why} = \"no source\";\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let acc = self.with_access(field);
            self.push(&format!(
                "    if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {why} = \"\";\n    }} else {{\n      {why} = \"not configured\";\n    }}\n"
            ));
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let ty = type_ident_from_id(&shape.id);
        let mut known = Vec::new();
        let mut required_checks = String::new();
        let mut type_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members {
                known.push(format!("{:?}", m.name));
                if m.required {
                    required_checks.push_str(&format!(
                        "      if (!({name:?} in parsed)) {{\n        throw new Error(`${{{label}}}: missing field {name}`);\n      }}\n",
                        name = m.name,
                    ));
                }
                // Scalar wire-type checks keep the strictness on par with the
                // Go decoder (which is typed); containers and refs decode as
                // the wire codec always has.
                let expected = match &m.target {
                    Tref::Prim(
                        Prim::String
                        | Prim::Uuid
                        | Prim::Timestamp
                        | Prim::Date
                        | Prim::Duration
                        | Prim::Bytes
                        | Prim::I64
                        | Prim::U64,
                    ) => Some(("string", "a string")),
                    Tref::Prim(
                        Prim::I8
                        | Prim::I16
                        | Prim::I32
                        | Prim::U8
                        | Prim::U16
                        | Prim::U32
                        | Prim::Float,
                    ) => Some(("number", "a number")),
                    Tref::Prim(Prim::Bool) => Some(("boolean", "a boolean")),
                    _ => None,
                };
                if let Some((ts_typeof, describe)) = expected {
                    let guard = if m.required {
                        String::new()
                    } else {
                        format!(
                            "record[{name:?}] !== undefined && record[{name:?}] !== null && ",
                            name = m.name
                        )
                    };
                    type_checks.push_str(&format!(
                        "      if ({guard}{present}typeof record[{name:?}] !== {ts_typeof:?}) {{\n        throw new Error(`${{{label}}}: field {name} must be {describe}`);\n      }}\n",
                        present = if m.required {
                            format!("{name:?} in parsed && ", name = m.name)
                        } else {
                            String::new()
                        },
                        name = m.name,
                    ));
                }
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            let en = error_names();
            format!(
                "      const vs = validate{ty}(decoded);\n      if (vs.length > 0) {{\n        throw new {validation}(vs);\n      }}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let block = format!(
            "    if ({why} !== \"\") {{\n      const raw = {lookup};\n      if (raw !== undefined) {{\n\
             \x20       let parsed: unknown;\n\
             \x20       try {{\n          parsed = JSON.parse(raw);\n        }} catch (e) {{\n          throw new Error(`${{{label}}}: ${{String(e)}}`);\n        }}\n\
             \x20       if (typeof parsed !== \"object\" || parsed === null) {{\n          throw new Error(`${{{label}}}: expected an object`);\n        }}\n\
             \x20       const record = parsed as Record<string, unknown>;\n\
             {required}\
             \x20       for (const key of Object.keys(parsed)) {{\n          if (![{known}].includes(key)) {{\n            throw new Error(`${{{label}}}: unknown field ${{key}}`);\n          }}\n        }}\n\
             {types}\
             \x20       const decoded = decode{ty}(parsed);\n\
             {validate}\
             \x20       {dest} = decoded;\n\
             \x20       {why} = \"\";\n\
             \x20     }} else {{\n        {why} = {miss};\n      }}\n    }}\n",
            known = known.join(", "),
            required = indent2(&required_checks),
            types = indent2(&type_checks),
            validate = indent2(&validate),
        );
        self.push(&block);
    }

    /// A map/list field: an explicit `@arg`/`@with` value passes typed, an env
    /// value decodes as JSON whole.
    fn emit_json(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("    {dest} = {};\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        self.push(&format!("    let {why} = \"no source\";\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let acc = self.with_access(field);
            self.push(&format!(
                "    if ({acc} !== undefined) {{\n      {dest} = {acc};\n      {why} = \"\";\n    }} else {{\n      {why} = \"not configured\";\n    }}\n"
            ));
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let ty = ts_type(&field.target);
        let block = format!(
            "    if ({why} !== \"\") {{\n      const raw = {lookup};\n      if (raw !== undefined) {{\n        try {{\n          {dest} = JSON.parse(raw) as {ty};\n        }} catch (e) {{\n          throw new Error(`${{{label}}}: ${{String(e)}}`);\n        }}\n        {why} = \"\";\n      }} else {{\n        {why} = {miss};\n      }}\n    }}\n"
        );
        self.push(&block);
    }

    fn emit_config(&mut self, field: &EntryField, shape: &Shape) {
        let ShapeKind::Config { fields } = &shape.kind else {
            return;
        };
        let ty = type_ident_from_id(&shape.id);
        let dest = self.ident(&field.name);
        let mut block = format!("    {{\n      const composed = {{}} as {ty};\n");
        for member in fields {
            let member_dest = format!("composed.{}", field_camel(&member.name, self.config));
            let bind = field.binds.iter().find(|b| b.field == member.name);
            let mut member_stmts = String::new();
            if let Some(bind) = bind {
                let head = bind.source.first().cloned().unwrap_or_default();
                let expr = self.path_expr(&bind.source);
                if self.guaranteed(&head) {
                    member_stmts.push_str(&format!("    {member_dest} = {expr};\n"));
                } else {
                    member_stmts.push_str(&format!(
                        "    if ({head_why} === \"\") {{\n      {member_dest} = {expr};\n    }} else {{\n{fallback}    }}\n",
                        head_why = why_var(&head),
                        fallback = self
                            .member_sources_stmts(member, &member_dest)
                            .lines()
                            .map(|l| format!("  {l}\n"))
                            .collect::<String>(),
                    ));
                }
            } else {
                member_stmts.push_str(&self.member_sources_stmts(member, &member_dest));
            }
            block.push_str(
                &member_stmts
                    .lines()
                    .map(|l| format!("  {l}\n"))
                    .collect::<String>(),
            );
        }
        block.push_str(&format!("      {dest} = composed;\n    }}\n"));
        self.push(&block);
    }

    fn member_sources_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let stub = EntryField {
            name: member.name.clone(),
            target: member.target.clone(),
            sources: member.sources.clone(),
            format: None,
            transforms: vec![],
            select: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        };
        let guaranteed = stub.sources.iter().any(|s| matches!(s, Source::Default(_)));
        if guaranteed {
            self.chain_cascade(&stub, dest)
        } else {
            let mut out = String::new();
            for source in &stub.sources {
                if let Source::Env(name) = source {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    out.push_str(&format!(
                        "    {{\n      const v = {lookup};\n      if (v !== undefined) {{\n{parse}      }}\n    }}\n",
                        parse = self.env_parse(&stub, dest, &label),
                    ));
                }
            }
            out
        }
    }
}

fn indent2(block: &str) -> String {
    block
        .lines()
        .map(|l| format!("  {l}\n"))
        .collect::<String>()
}

fn prim_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::rendered;
    use crate::ir::decode_model;

    fn fixture_module() -> Module {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ir-schema/fixtures/entries_client.json"
        ));
        let model = decode_model(text).expect("fixture decodes");
        model.modules.into_iter().next().expect("one module")
    }

    fn with_descriptors(mut module: Module) -> Module {
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
                for op in operations {
                    op.traits.push(crate::ir::Trait {
                        id: "wire_descriptor".into(),
                        value: serde_json::json!({"http_method": "POST", "uri": "/notes/{id}"}),
                    });
                }
            }
        }
        module
    }

    fn text(module: &Module) -> String {
        rendered(&entry_decls(module, &ts_casing()), &TsRules)
    }

    #[test]
    fn the_entry_class_replaces_the_generic_client_surface() {
        let module = with_descriptors(fixture_module());
        let out = text(&module);
        // The class takes @arg positionally and @with as an optional config
        // object; the Settings expose the resolved fields and transport slots.
        assert!(out.contains("export class Client {"));
        assert!(out.contains("constructor(apiKey: string, config: ClientConfig = {}) {"));
        assert!(out.contains("export interface Settings {"));
        assert!(out.contains("  fetch?: typeof fetch;"));
        assert!(out.contains("  transport?: CanonicalTransport;"));
        assert!(out.contains("  headers: Record<string, string>;"));
        assert!(out.contains("export interface ClientConfig {"));
        assert!(out.contains("  clientName?: string;"));
        // Construction-only config interface.
        assert!(out.contains("export interface Conf {"));
        // The descriptor is embedded verbatim and the method maps the outcome.
        assert!(out.contains("const saveNoteDescriptor: WireDescriptor = JSON.parse("));
        assert!(out.contains("async saveNote(input: Note): Promise<Note> {"));
        assert!(out.contains("throw new TransportError(outcome.cause);"));
        assert!(out.contains("decodeSaveNoteError(outcome.status, outcome.body)"));
    }

    #[test]
    fn the_resolution_mirrors_the_go_spelling() {
        let module = with_descriptors(fixture_module());
        let out = text(&module);
        assert!(out.contains("s.apiKey = apiKey;"));
        assert!(out.contains("s.clientName = \"demo\";"));
        assert!(out.contains("s.clientKey = strUpperSnake((s.clientName).trim());"));
        assert!(out.contains("switch (s.endpointVersion) {"));
        assert!(out.contains("case \"v1\": {"));
        assert!(out.contains("endpointWhy = \"endpoint_v1 <- \" + endpointV1Why;"));
        assert!(out.contains("composed.apiKey = s.apiKey;"));
        // Values freeze under canonical dotted names; bigints narrow, the
        // duration flows in milliseconds.
        assert!(out.contains("values[\"settings.api_key\"] = s.settings.apiKey;"));
        assert!(out.contains("values[\"timeout\"] = durationToMs(String(s.timeout));"));
        // Entry construction leaves baseUrl empty: the endpoint resolves per
        // operation from the descriptor's ref.
        assert!(out.contains(
            "this.options = { baseUrl: \"\", fetch: s.fetch, transport: s.transport, headers: s.headers, values };"
        ));
    }

    #[test]
    fn the_settings_bridge_wires_client_init_by_mutation() {
        let mut module = with_descriptors(fixture_module());
        module.extensions = vec![crate::ir::Extension {
            name: "client_init".into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            bindings: [("ts".to_string(), "ext/ts/init.ts#initSettings".to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        }];
        let out = text(&module);
        assert!(out.contains("function wrapClientInit(settings: Settings): void {"));
        assert!(out.contains("initSettings(settings);"));
        assert!(out.contains("wrapClientInit(s);"));
        assert!(out.contains("throw new ContractError(\"client_init\", e);"));
        // Bridge before validation: init runs before the consumed-chain check.
        let init = out.find("wrapClientInit(s);").unwrap();
        let require = out.find("throw new Error(\"endpoint <- \"").unwrap();
        assert!(init < require);
    }

    #[test]
    fn structured_sources_decode_strictly_with_context() {
        let mut module = with_descriptors(fixture_module());
        // Attach a structured field referencing a wire struct with a required
        // member and a check.
        let creds = Shape {
            id: "notes#credentials".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![
                    Member {
                        name: "token".into(),
                        target: Tref::Prim(Prim::String),
                        required: true,
                        default: None,
                        constraints: vec![crate::ir::Constraint::Length {
                            min: Some(1),
                            max: None,
                        }],
                        traits: vec![],
                    },
                    Member {
                        name: "account_id".into(),
                        target: Tref::Prim(Prim::String),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                ],
            },
            traits: vec![],
        };
        module.shapes.push(creds);
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                fields.push(EntryField {
                    name: "creds".into(),
                    target: Tref::Ref {
                        id: "notes#credentials".into(),
                        args: vec![],
                    },
                    sources: vec![Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into()))],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
            }
        }
        let out = text(&module);
        assert!(out.contains("missing field token"));
        assert!(out.contains("unknown field ${key}"));
        assert!(out.contains("const decoded = decodeCredentials(parsed);"));
        assert!(out.contains("const vs = validateCredentials(decoded);"));
        assert!(out.contains("throw new ValidationError(vs);"));
        // Required members are checked before unknown fields (Go's order), and
        // scalar wire types are checked so a mistyped member fails the same
        // way the typed Go decode does.
        let required = out.find("missing field token").unwrap();
        let unknown = out.find("unknown field ${key}").unwrap();
        assert!(required < unknown);
        assert!(out.contains("field token must be a string"));
    }

    #[test]
    fn a_bespoke_stub_keeps_the_declared_signature() {
        // No descriptor on the op (the fixture is pre-protocol): the stub
        // still takes the declared input.
        let module = fixture_module();
        let out = text(&module);
        assert!(out.contains("async saveNote(input: Note): Promise<Note> {"));
        assert!(out.contains("operation has no transport binding"));
    }

    #[test]
    fn a_guaranteed_chain_reads_each_env_variable_once() {
        let mut module = with_descriptors(fixture_module());
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                fields.push(EntryField {
                    name: "my_region".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![
                        Source::Env(EnvName::Name("MY_REGION".into())),
                        Source::Default(serde_json::json!("us")),
                    ],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
            }
        }
        let out = text(&module);
        assert_eq!(out.matches("readEnv(\"MY_REGION\")").count(), 1);
        assert!(out.contains("let myRegionSet = false;"));
        assert!(out.contains("if (!myRegionSet) {"));
    }

    #[test]
    fn a_module_without_entries_emits_nothing() {
        let module = Module {
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![],
        };
        assert!(entry_decls(&module, &ts_casing()).is_empty());
    }
}

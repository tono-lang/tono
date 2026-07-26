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
    companion_name, module_entries, op_local_name, ref_is_enum, EntryModel, FieldShape,
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
        Tref::Prim(Prim::Bytes) => "new Uint8Array()".into(),
        Tref::Map(_, _) => "{}".into(),
        Tref::List(_) => "[]".into(),
        // A named shape starts from an empty object (mirroring Go's zero
        // struct); only string-branded leaves start from the empty string.
        Tref::Ref { .. } => format!("{{}} as {}", ts_type(t)),
        _ => format!("\"\" as {}", ts_type(t)),
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
        // A bound contract/constraint gets its boundary wrapper here too;
        // without this an entry-only module would leave the binding
        // silently unused (the loose-op client is what emits it otherwise).
        let mut contract_refs = Vec::new();
        let contracts = crate::codegen::targets::typescript::client::contract_wrappers(
            &bound,
            module,
            &mut contract_refs,
        );
        if !contracts.is_empty() {
            decls.push(Decl::raw_with(contracts, contract_refs));
        }
        // In a mixed module the loose client owns wrapClientInit (with its
        // Partial<ClientOptions> signature); generation rejects that
        // combination upstream, so the bridge is only emitted here.
        decls.extend(client_init_wrapper(&bound, &entries, multi, module));
    }
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

/// The class per entry: constructor (resolution, bridge, validation, frozen
/// options) plus one async method per operation.
#[allow(clippy::too_many_arguments)]
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
    if !entry.with_fields().is_empty() {
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

    // Consumed chains must hold a value once construction finishes. Every
    // check reads the resolved value (client_init ran already, bespoke wins),
    // so the why-reason only decorates the error.
    for path in entry.consumed_field_paths() {
        let Some(head) = path.first() else {
            continue;
        };
        let Some(field) = entry.fields.iter().find(|f| f.name == *head) else {
            continue;
        };
        let shape = entry.field_shape(field, module);
        if path.len() > 1 && matches!(shape, FieldShape::Config(_) | FieldShape::Structured(_)) {
            // A consumed member of a composed or decoded field: the leaf value
            // itself must be there (there is no member-level why to report).
            let leaf = entry.path_type(&path, module);
            if !string_like(&leaf) {
                continue;
            }
            body.push_str(&format!(
                "    if ((s.{head_ident}.{member_ident} ?? {zero}) === {zero}) {{\n      throw new Error(\"{name}: no value\");\n    }}\n",
                head_ident = field_camel(head, config),
                member_ident = field_camel(&path[1], config),
                zero = cast_string(&leaf, "\"\""),
                name = path.join("."),
            ));
            continue;
        }
        if !matches!(shape, FieldShape::Scalar) || entry.is_guaranteed(field) {
            continue;
        }
        if string_like(&field.target) {
            body.push_str(&format!(
                "    if (s.{ident} === {zero}) {{\n      throw new Error(\"{name} <- \" + ({why} || \"no value\"));\n    }}\n",
                ident = field_camel(head, config),
                zero = cast_string(&field.target, "\"\""),
                why = why_var(head),
                name = head,
            ));
        } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
            body.push_str(&format!(
                "    if (s.{ident}.length === 0) {{\n      throw new Error(\"{name} <- \" + ({why} || \"no value\"));\n    }}\n",
                ident = field_camel(head, config),
                why = why_var(head),
                name = head,
            ));
        } else if matches!(
            field.target,
            Tref::Prim(
                Prim::I8
                    | Prim::I16
                    | Prim::I32
                    | Prim::I64
                    | Prim::U8
                    | Prim::U16
                    | Prim::U32
                    | Prim::U64
                    | Prim::Float
            )
        ) {
            // A numeric zero can be a legitimate resolved value, so only the
            // combination (chain reported absent, still zero after the
            // bridge) fails construction. A bool has no absent-vs-zero
            // distinction at all, so it carries no require.
            let zero = if matches!(field.target, Tref::Prim(Prim::I64 | Prim::U64)) {
                "0n"
            } else {
                "0"
            };
            body.push_str(&format!(
                "    if ({why} !== \"\" && s.{ident} === {zero}) {{\n      throw new Error(\"{name} <- \" + {why});\n    }}\n",
                ident = field_camel(head, config),
                why = why_var(head),
                name = head,
            ));
        }
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
            // The check reads the value bespoke left in place (client_init
            // ran already, bespoke wins), so presence is judged off the value
            // itself, never the declared chain's why-reason.
            let guard = if entry.is_guaranteed(field) || composed {
                line.condition.clone()
            } else if string_like(&field.target) {
                format!(
                    "s.{} !== {} && {}",
                    field_camel(&field.name, config),
                    cast_string(&field.target, "\"\""),
                    line.condition
                )
            } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
                format!(
                    "s.{}.length !== 0 && {}",
                    field_camel(&field.name, config),
                    line.condition
                )
            } else if matches!(
                field.target,
                Tref::Prim(
                    Prim::I8
                        | Prim::I16
                        | Prim::I32
                        | Prim::I64
                        | Prim::U8
                        | Prim::U16
                        | Prim::U32
                        | Prim::U64
                        | Prim::Float
                )
            ) {
                // A numeric zero can be a legitimate resolved value, so the
                // check only skips when the chain reported absent AND the
                // bridge left the zero in place (same rule as the requires).
                let zero = if matches!(field.target, Tref::Prim(Prim::I64 | Prim::U64)) {
                    "0n"
                } else {
                    "0"
                };
                format!(
                    "({why} === \"\" || s.{ident} !== {zero}) && {}",
                    line.condition,
                    why = why_var(&field.name),
                    ident = field_camel(&field.name, config),
                )
            } else {
                line.condition.clone()
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
        // An enum-typed leaf is a branded string wherever it sits (a field or
        // a composed/structured member): it freezes like any other scalar the
        // descriptor's refs can name.
        let scalar_ref = ref_is_enum(vp.target, module);
        if value_expr(&vp, config, scalar_ref).is_none() {
            continue;
        }
        // A member of a structured draft may be undefined (the draft starts
        // from an empty object); reading it through its zero keeps the frozen
        // values and the presence guard aligned with Go's zero struct. A
        // string-like member falls back to the empty string (its Go zero),
        // never to the draft's empty-object spelling.
        let expr = if vp.member.is_some() {
            let zero = if string_like(vp.target) {
                cast_string(vp.target, "\"\"")
            } else {
                zero_value(vp.target)
            };
            format!("(s.{} ?? {})", access(&vp, config), zero)
        } else {
            format!("s.{}", access(&vp, config))
        };
        let assign = if let Tref::Prim(Prim::Duration) = vp.target {
            helpers.duration_ms = true;
            format!(
                "    try {{\n      values[{path:?}] = durationToMs(String({expr}));\n    }} catch {{\n      throw new Error(`{path}: invalid duration ${{JSON.stringify(String({expr}))}}`);\n    }}\n",
                path = vp.path,
            )
        } else {
            format!(
                "    values[{path:?}] = {value};\n",
                path = vp.path,
                value = value_cast(vp.target, &expr),
            )
        };
        match presence_guard(entry, &vp, &expr) {
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
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    passes_hooks: bool,
    refs: &mut Vec<Symbol>,
) -> String {
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
        Some(t) => {
            // A 64-bit integer (or a container holding one) rides the wire
            // as strings: the parsed body runs through the same decode the
            // codecs use so the method returns bigints, not raw JSON shapes.
            let decode = crate::codegen::targets::typescript::codecs::decode_expr(
                "JSON.parse(outcome.body)",
                t,
            );
            if decode == "JSON.parse(outcome.body)" {
                format!("    return {decode} as {ret};")
            } else {
                refs.push(module_symbol(&en.decode, module));
                format!(
                    "    try {{\n      return {decode} as {ret};\n    }} catch {{\n      {t}\n    }}",
                    t = throw(format!(
                        "new {}(\"$\", {ret:?}, outcome.body)",
                        en.decode
                    )),
                )
            }
        }
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

mod checks;
mod resolve;
mod surface;
#[cfg(test)]
mod tests;

use checks::{access, presence_guard, value_cast, value_expr};
use resolve::Resolver;
use surface::*;

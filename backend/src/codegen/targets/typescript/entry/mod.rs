//! The TypeScript entry client: the SDK construction surface an entry
//! declares, spelled as an exported class per entry (it replaces the generic
//! `HttpClient` for a module that declares one). The constructor takes the
//! `@arg` fields positionally and the `@with` fields as an optional config
//! object, resolves every declared source chain top-down, parses env values by
//! the field's type at the boundary (the error names the variable and the
//! type), lowers a match selection to a `switch`, decodes a structured source
//! strictly, composes a config through `@bind`, runs the bound `client_init`
//! hook over the resolved `Settings` (bespoke wins, by mutation), and
//! validates last. Each operation method resolves its own `wire` binding's
//! ref positions (endpoint, headers, timeout, retry) against the resolved
//! fields directly and builds the request inline; no runtime interprets a
//! descriptor.

use std::collections::{BTreeMap, BTreeSet};

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    deprecated_of, doc_of, field_ident, rename_of, type_ident_from_id, wire_key,
};
use crate::codegen::entries::{companion_name, op_local_name, plan, EntryModel, FieldShape};
use crate::codegen::extensions::{hook_binding, impl_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_binding};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::{type_expr_of, TsVal, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{EntryField, EnvName, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Tref};

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

/// An entry field's public identifier, honoring its `@rename(typescript)`
/// override. Config members and path tails keep the plain [`field_camel`].
fn field_camel_ren(name: &str, rename: Option<&str>, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, rename)
}

/// The JSDoc `@doc`/`@deprecated` block for an entry field's public surface (a
/// Settings or config-object property), indented and newline-terminated, or
/// empty when the field carries neither trait.
fn field_doc(traits: &[crate::ir::Trait], indent: &str) -> String {
    crate::codegen::doc::jsdoc_block(
        doc_of(traits).as_deref(),
        deprecated_of(traits).as_deref(),
        indent,
    )
}

/// A reference to a declaration of the SDK's shared support group.
fn support_symbol(name: &str) -> Symbol {
    Symbol::imported(name, crate::codegen::group::ROOT_SUPPORT, name)
}

pub(super) fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// The companion-file symbols a declared type drags into the serde file: the
/// branded well-known aliases and any named shape.
fn type_refs(t: &Tref, module: &Module) -> Vec<Symbol> {
    match t {
        // A branded well-known type is the SDK's, not the module's.
        Tref::Prim(Prim::Timestamp) => vec![support_symbol("Timestamp")],
        Tref::Prim(Prim::Date) => vec![support_symbol("LocalDate")],
        Tref::Prim(Prim::Duration) => vec![support_symbol("Duration")],
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

pub use plan::EntryEmission;

/// Emit a module's entries: the Settings and config interfaces, the descriptor
/// constants, the class and its methods, grouped per entry so the construction
/// surface reads together; the wrappers and helpers every entry shares stay in
/// the module's internal group, emitted once.
pub fn emit(module: &Module, config: &CasingConfig) -> EntryEmission {
    let Some((entries, multi, bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
        return EntryEmission::empty();
    };
    let tested = crate::codegen::declared_tests::entries_with_tests(module);
    let mut helpers = Helpers::default();
    let mut decls = Vec::new();
    decls.extend(config_interfaces(module, config));
    decls.extend(transport_hook_wrappers(&bound, module));
    // A bound contract/constraint gets its boundary wrapper here too, or the
    // binding would go silently unused.
    let mut contract_refs = Vec::new();
    let contracts = crate::codegen::targets::typescript::client::contract_wrappers(
        &bound,
        module,
        &mut contract_refs,
    );
    if !contracts.is_empty() {
        decls.push(Decl::raw_with(contracts, contract_refs));
    }
    decls.extend(client_init_wrapper(&bound, &entries, multi, module));
    decls.extend(plan::output_decode_decls(
        &entries,
        module,
        |op| wire_binding(op).is_some() || impl_binding(&bound, &op.id).is_some_and(|b| b.raw),
        |shape| decode::output_decode_decl(shape, module),
    ));
    let mut per_entry = Vec::new();
    for entry in &entries {
        let n = names(entry, multi);
        let has_tests = tested.contains(entry.name);
        let mut own = vec![settings_interface(entry, &n, config, module)];
        own.extend(config_object_interface(entry, &n, config, module));
        own.extend(impl_op::seam_decls(entry, &n, module, &bound, has_tests));
        own.extend(class_decl(
            entry,
            &n,
            module,
            config,
            &bound,
            &mut helpers,
            multi,
            has_tests,
        ));
        own.extend(discriminator_decls_for(entry, &n, module, &bound));
        per_entry.push((entry.name.to_string(), own));
    }
    EntryEmission {
        shared: decls,
        per_entry,
    }
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
    has_tests: bool,
) -> Vec<Decl> {
    let en = error_names();
    let mut resolve_fns: Vec<Decl> = Vec::new();
    let mut refs = vec![
        support_symbol("ClientOptions"),
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
        .map(|f| {
            format!(
                "{}: {}",
                plan::arg_camel(&f.name, &f.traits, LANG),
                ts_type(&f.target)
            )
        })
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
                field_camel_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
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
        resolve_fns: &mut resolve_fns,
        multi,
    };
    // The constructor is nested one level deeper here than in Go's flat
    // function (a class body wraps it), so the plan renders one indent unit
    // further in to land at the same column as this scaffold's own lines.
    let fields = plan::emit_fields(entry, module, &mut r, 2);
    if !fields.is_empty() {
        plan::push_gap(r.body);
        r.body.push_str(&fields);
    }

    if hook_binding(bound, "client_init").is_some() && !multi {
        plan::push_gap(&mut body);
        body.push_str("    wrapClientInit(s);\n");
    }

    // Consumed chains must hold a value once construction finishes. The shared
    // plan picks which fields need a check (client_init ran already, bespoke
    // wins, so each check reads the resolved value); this target spells them.
    {
        let mut r = Resolver {
            entry,
            module,
            config,
            helpers,
            body: &mut body,
            resolve_fns: &mut resolve_fns,
            multi,
        };
        let requires = plan::build_requires(entry, module, &mut r);
        let text = plan::render(&requires, 2, &r);
        if !text.is_empty() {
            plan::push_gap(r.body);
            r.body.push_str(&text);
        }
    }

    // Declared validation runs last, over what bespoke left in place.
    let mut guards = String::new();
    for field in &entry.fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = crate::codegen::entries::field_as_member(field);
        for line in validation::guard_lines(&[member], &TsVal, "s.", config, LANG) {
            // The check reads the value bespoke left in place (client_init ran
            // already, bespoke wins), so presence is judged off the value
            // itself, never the declared chain's error var. A numeric zero can
            // be a legitimately resolved value, so its guard only skips when the
            // chain reported absent AND the bridge left the zero in place.
            let guard = match plan::presence_kind(field, entry, module) {
                plan::Presence::Always => line.condition.clone(),
                plan::Presence::String => format!(
                    "s.{} !== {} && {}",
                    field_camel_ren(
                        &field.name,
                        rename_of(&field.traits, LANG).as_deref(),
                        config
                    ),
                    cast_string(&field.target, "\"\""),
                    line.condition
                ),
                plan::Presence::Bytes => format!(
                    "s.{}.length !== 0 && {}",
                    field_camel_ren(
                        &field.name,
                        rename_of(&field.traits, LANG).as_deref(),
                        config
                    ),
                    line.condition
                ),
                plan::Presence::Numeric => {
                    let zero = if matches!(field.target, Tref::Prim(Prim::I64 | Prim::U64)) {
                        "0n"
                    } else {
                        "0"
                    };
                    format!(
                        "({err} === undefined || s.{ident} !== {zero}) && {}",
                        line.condition,
                        err = err_var(&field.name),
                        ident = field_camel_ren(
                            &field.name,
                            rename_of(&field.traits, LANG).as_deref(),
                            config
                        ),
                    )
                }
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
        plan::push_gap(&mut body);
        body.push_str(&format!(
            "    const violations: {violation}[] = [];\n{guards}    if (violations.length > 0) {{\n      throw new {validation}(violations);\n    }}\n",
            violation = en.violation,
            validation = en.validation,
        ));
    }

    // Every distinct `@timeout` field path any operation actually declares
    // (usually one, but a multi-op entry could bind more than one) becomes
    // its own private field: the constructor converts Duration to
    // milliseconds once, eagerly, so a malformed value still fails
    // construction (ConfigError) rather than surfacing at the first call
    // that happens to need it. `@retry`/`endpoint`/`@header`/path-template
    // positions all read the typed Settings (or, for retry, the resolved
    // numeric field) directly at the call site (see `op_method`'s
    // `field_expr`), so there is no runtime values map left to freeze into.
    let timeout_paths: BTreeMap<String, Vec<String>> = entry
        .operations
        .iter()
        .filter_map(|op| match &op.kind {
            ShapeKind::Operation { wire: Some(w), .. } => {
                w.timeout.as_ref().map(|p| (p.join("."), p.clone()))
            }
            _ => None,
        })
        .collect();
    let mut timeout_field_decls = String::new();
    let mut timeout_field_by_path: BTreeMap<String, String> = BTreeMap::new();
    for vp in entry.value_paths(module) {
        let Some(segments) = timeout_paths.get(&vp.path) else {
            continue;
        };
        let field_name = timeout_field_name(entry, config, segments);
        timeout_field_decls.push_str(&format!("  private readonly {field_name}: number;\n"));
        // A member of a structured draft may be undefined (the draft starts
        // from an empty object); reading it through its zero keeps this
        // aligned with the constructor's other member reads.
        let expr = if vp.member.is_some() {
            format!(
                "(s.{} ?? {})",
                access(&vp, config),
                cast_string(vp.target, "\"\"")
            )
        } else {
            format!("s.{}", access(&vp, config))
        };
        helpers.duration_ms = true;
        let fail = config_error(&format!(
            "`{path}: invalid duration ${{JSON.stringify(String({expr}))}}`",
            path = vp.path,
        ));
        let assign = format!(
            "    try {{\n      this.{field_name} = durationToMs(String({expr}));\n    }} catch {{\n      {fail}\n    }}\n",
        );
        match presence_guard(entry, &vp, &expr) {
            Some(guard) => body.push_str(&format!(
                "    if ({guard}) {{\n  {assign}    }} else {{\n      this.{field_name} = 0;\n    }}\n",
            )),
            None => body.push_str(&assign),
        }
        timeout_field_by_path.insert(vp.path.clone(), field_name);
    }

    // The frozen client options: the per-operation endpoint resolves from the
    // wire binding, so the options carry only the transport slots bridged off
    // the resolved Settings.
    body.push_str(
        "    this.settings = s;\n    this.options = { fetch: s.fetch, transport: s.transport, headers: s.headers };\n",
    );
    // The mutually-exclusive transport slots are rejected at construction (as Go
    // does in New), so a misconfigured client fails to build instead of failing
    // obscurely on its first call.
    body.push_str("    assertExclusiveTransport(this.options);\n");

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
            refs.push(support_symbol("Hooks"));
            format!(
                "  private readonly hooks: Hooks = {{ {} }};\n",
                slots.join(", ")
            )
        }
    };
    // The already-converted, already-validated millisecond value for a
    // `@timeout` field path, read off its private field rather than
    // re-deriving `durationToMs` at every call site.
    let timeout_field_expr =
        |path: &[String]| format!("this.{}", timeout_field_by_path[&path.join(".")]);
    let mut methods = String::new();
    for op in entry.operations {
        methods.push_str(&op_method(
            n,
            op,
            module,
            config,
            entry,
            bound,
            &timeout_field_expr,
            &mut refs,
        ));
        methods.push('\n');
    }

    // The construction seam the generated tests use, emitted only when the
    // entry declares tests so the shipped surface stays clean. The real
    // construction path runs first (resolution, client_init, validation),
    // then the canonical transport replaces whatever construction resolved, so
    // a test answers canonically without a server. ClientOptions spells its
    // slots readonly, so the swap goes through a mutable view of the frozen
    // options object (the object itself is a plain literal).
    let for_test = if has_tests {
        refs.push(support_symbol("HttpTransport"));
        let sig_params = if params.is_empty() {
            String::new()
        } else {
            format!(", {}", params.join(", "))
        };
        let mut pass: Vec<String> = args
            .iter()
            .map(|f| plan::arg_camel(&f.name, &f.traits, LANG))
            .collect();
        if !entry.with_fields().is_empty() {
            pass.push("config".to_string());
        }
        format!(
            "  // forTest is the constructor plus the transport seam the generated\n\
             \x20 // tests construct through: the real construction path runs first, then\n\
             \x20 // the canonical transport wins over anything bespoke.\n\
             \x20 static forTest(seam: {{ transport: HttpTransport }}{sig_params}): {client} {{\n\
             \x20   const client = new {client}({pass});\n\
             \x20   const options = client.options as {{ transport?: HttpTransport; fetch?: typeof fetch }};\n\
             \x20   options.transport = seam.transport;\n\
             \x20   options.fetch = undefined;\n\
             \x20   return client;\n\
             \x20 }}\n\n",
            client = n.client,
            pass = pass.join(", "),
        )
    } else {
        String::new()
    };

    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let text = format!(
        "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
         // constructor takes the @arg values positionally and the @with values as a\n\
         // config object; construction resolves the declared sources, runs the\n\
         // client_init bridge, validates, and freezes the runtime options.\n\
         export class {client} {{\n\
         \x20 private readonly settings: {settings};\n\
         \x20 private readonly options: ClientOptions;\n\
         {timeout_field_decls}\
         {hooks_field}\
         \x20 constructor({params}) {{\n{body}  }}\n\n{for_test}{methods}}}",
        client = n.client,
        entry_name = entry.name,
        settings = n.settings,
        params = params.join(", "),
    );
    // Any construction-time failure (a bad env value, a malformed blob, an
    // unmatched select, an unresolved consumed chain) throws ConfigError, so
    // the class file imports that category when the body emits one.
    if text.contains(&format!("new {}(", en.config)) {
        refs.push(module_symbol(&en.config, module));
    }
    resolve_fns.push(Decl::raw_with(text, refs));
    resolve_fns
}

/// One operation method: builds the request from the resolved wire binding,
/// calls the transport inline, and maps the outcome onto the taxonomy.
#[allow(clippy::too_many_arguments)]
fn op_method(
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    entry: &EntryModel<'_>,
    bound: &[BoundExtension<'_>],
    timeout_field_expr: &dyn Fn(&[String]) -> String,
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

    // A constrained input is validated before it leaves the process, so a bad
    // request surfaces as a ValidationError instead of a server round trip (or
    // a call into bespoke code that would have to reject it again).
    let mut validate_block = String::new();
    if let Some(Tref::Ref { id, .. }) = input {
        if let Some(shape) = module
            .shapes
            .iter()
            .find(|s| s.id == *id)
            .filter(|s| validation::shape_has_checks(s))
        {
            let ty = type_ident_from_id(&shape.id);
            refs.push(module_symbol(&format!("validate{ty}"), module));
            validate_block = format!(
                "    const invalid = validate{ty}(input);\n    if (invalid) {{\n      {t}\n    }}\n",
                t = throw("invalid".to_string()),
            );
        }
    }

    let Some(wire) = wire_binding(op) else {
        // No protocol binding: the operation is implemented by bespoke sources
        // the frontend proved are bound, and the generator gate proved are bound
        // for this target.
        return impl_op::method(impl_op::Method {
            n,
            op,
            module,
            name: &name,
            param: &param,
            ret: &ret,
            input_expr: &input_expr,
            output,
            validate_block: &validate_block,
            binding: impl_binding(bound, &op.id),
            throw: &throw,
            discriminator: &discriminator_name(n, op),
            refs,
        });
    };

    let has_declared_errors = !declared_errors(op, module).is_empty();
    let discriminator = discriminator_name(n, op);
    let error_line = if has_declared_errors {
        throw(format!("{discriminator}(outcome.status, outcome.body)"))
    } else {
        refs.push(module_symbol(&en.api, module));
        throw(format!("new {}(outcome.status, outcome.body)", en.api))
    };
    let success_block = decode::success_block(output, module, &ret, &throw, refs);
    let before_request_bound = hook_binding(bound, "before_request").is_some();
    let after_response_bound = hook_binding(bound, "after_response").is_some();
    let http_method = wire.method.clone();
    // The frontend guarantees an endpoint/@header/path-template/@retry field
    // reference resolves to a value already sitting, typed, on the resolved
    // Settings — read it there directly instead of through a runtime bag.
    // @timeout is the one exception: its field converts (Duration to
    // milliseconds) rather than passing through, so it reads the private,
    // already-converted field the constructor built (see class_decl).
    let field_expr = |path: &[String]| field_path_expr(entry, config, path, "this.settings");
    // A param-member reference resolves the same way, off the op's own
    // parameter type instead of the entry: a same-module structure member
    // reads as a typed `input.field`; a cross-module parameter type is not
    // chased here, so the caller falls back to the decoded record.
    let param_access = |seg: &str| -> Option<String> {
        let member = crate::codegen::ops::param_member(module, input, seg)?;
        Some(format!("input.{}", field_ident(member, config, LANG)))
    };
    let transport_body = transport::op_call(
        wire,
        &http_method,
        &input_expr,
        has_declared_errors,
        &discriminator,
        &error_line,
        &success_block,
        &en.transport,
        &throw,
        before_request_bound,
        after_response_bound,
        &field_expr,
        timeout_field_expr,
        &param_access,
        refs,
    );
    let doc = doc_of(&op.traits)
        .map(|d| format!("  // {}\n", d.replace('\n', "\n  // ")))
        .unwrap_or_default();
    format!(
        "{doc}  async {name}({param}): Promise<{ret}> {{\n\
         {validate_block}\
         {transport_body}\
         \x20 }}",
    )
}

use plan::err_var;

mod checks;
mod decode;
mod impl_op;
mod resolve;
mod surface;
#[cfg(test)]
mod tests;
pub(crate) mod transport;
pub(crate) mod transport_decls;
pub(crate) mod vector_tests;

use checks::{access, config_error, field_path_expr, presence_guard, timeout_field_name};
use resolve::Resolver;
use surface::*;
pub use surface::{casing_helpers, duration_helpers, env_helpers, resolution_helpers};

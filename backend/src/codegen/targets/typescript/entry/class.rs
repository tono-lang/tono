//! The class per entry: construction (resolution, bridge, validation, frozen
//! options) plus one async method per operation. Split out of `entry/mod.rs`
//! to stay under this repo's per-file line ceiling; further split into one
//! function per logical block so each one stays legible on its own.
//!
//! Construction is three steps. `newSettings` (a private static) resolves
//! every construction value no foreign call reaches and runs the requires
//! and the declared validation over them. The constructor (or, for an entry
//! with a foreign construction, the `create` factory) then runs the foreign
//! constructions in resolution order, each a call to its own resolver (see
//! `ext_resolver`), and `fromSettings` (a private static, on the async shape)
//! freezes the runtime values and builds the client. A generated test
//! composes the same steps with its fakes in place of the resolver calls,
//! reaching the private statics through element access, so no step carries
//! a branch deciding between production and test and nothing test-only is
//! exported.

use super::{
    access, cast_string, config_error, err_var, field_camel_ren, field_ts_type, module_symbol,
    op_method, presence_guard, rename_of, support_symbol, timeout_field_name, type_refs,
    zero_value, BoundExtension, CasingConfig, Decl, EntryModel, FieldShape, Helpers, Module, Names,
    Resolver,
};
use crate::codegen::entries::plan;
use crate::codegen::entries::{has_wire_ops, ValuePath};
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::types::TsVal;
use crate::codegen::validation;
use crate::codegen::{conventions::doc_of, conventions::type_ident_from_id};
use crate::ir::{EntryField, Prim, ShapeKind, Tref};
use std::collections::BTreeMap;

use crate::codegen::targets::typescript::types::LANG;

/// TransportError is only ever thrown from a wire operation's own call site
/// (`transport::op_call`, which already adds its own reference there); an
/// entry with no wire operation at all never reaches that code, so importing
/// the category unconditionally would ask for a class the taxonomy's own
/// liveness never generates in the first place.
fn has_wire_op(entry: &EntryModel<'_>) -> bool {
    entry
        .operations
        .iter()
        .any(|op| matches!(&op.kind, ShapeKind::Operation { wire: Some(_), .. }))
}

/// The class's starting import set: `ClientOptions` when the module goes
/// over the wire, `TransportError` only when a wire operation can throw it,
/// and every declared field's own type refs (plus its validator, for a
/// structured field with checks).
fn base_refs(entry: &EntryModel<'_>, module: &Module) -> Vec<Symbol> {
    let en = error_names();
    let mut refs = Vec::new();
    if has_wire_ops(module) {
        refs.push(support_symbol("ClientOptions"));
    }
    if has_wire_op(entry) {
        refs.push(module_symbol(&en.transport, module));
    }
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
    refs
}

/// The constructor's positional parameter list (`@arg` fields, then an
/// optional `@with` config object), and the same names as they are passed on
/// to `newSettings`.
fn ctor_params(entry: &EntryModel<'_>, n: &Names, module: &Module) -> (Vec<String>, Vec<String>) {
    let args = entry.args();
    let mut params: Vec<String> = args
        .iter()
        .map(|f| {
            format!(
                "{}: {}",
                plan::arg_camel(&f.name, &f.traits, LANG),
                field_ts_type(&f.target, module)
            )
        })
        .collect();
    let mut pass: Vec<String> = args
        .iter()
        .map(|f| plan::arg_camel(&f.name, &f.traits, LANG))
        .collect();
    if !entry.with_fields().is_empty() {
        params.push(format!("config: {} = {{}}", n.config));
        pass.push("config".to_string());
    }
    (params, pass)
}

/// The resolution of `fields` (a half of the construction split), then the
/// consumed-chain requires and the declared validation of those fields, as
/// the body lines of the function resolving them (two indent levels deep,
/// inside a class member), plus the standalone resolver functions the plan
/// split off. A check reads the head's error variable, so it sits in the
/// same function that resolved the head.
#[allow(clippy::too_many_arguments)]
fn resolution_steps(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    multi: bool,
    n: &Names,
    fields: &[&EntryField],
    refs: &mut Vec<Symbol>,
) -> (String, Vec<Decl>) {
    let mut body = String::new();
    let mut resolve_fns: Vec<Decl> = Vec::new();
    let keep: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let mut r = Resolver {
        entry,
        module,
        config,
        helpers,
        body: &mut body,
        resolve_fns: &mut resolve_fns,
        multi,
        n,
    };
    // The step is nested one level deeper here than in Go's flat function
    // (a class body wraps it), so the plan renders one indent unit further
    // in to land at the same column as this scaffold's own lines.
    let rendered = plan::emit_fields_of(fields, entry, module, &mut r, 2);
    if !rendered.is_empty() {
        plan::push_gap(r.body);
        r.body.push_str(&rendered);
    }
    let requires = plan::build_requires_for(entry, module, &mut r, &|head| keep.contains(&head));
    let text = plan::render(&requires, 2, &r);
    if !text.is_empty() {
        plan::push_gap(r.body);
        r.body.push_str(&text);
    }
    let guards = validation_guards_block(entry, module, config, fields, refs);
    if !guards.is_empty() {
        plan::push_gap(&mut body);
        body.push_str(&guards);
    }
    (body, resolve_fns)
}

/// One declared-validation guard's condition, read off the field's resolved
/// value, so presence is judged off the value itself, never the declared
/// chain's error var. A numeric zero can be a legitimately resolved value, so
/// its guard only skips when the chain reported absent AND the resolved
/// value is still zero.
fn guard_condition(
    field: &EntryField,
    config: &CasingConfig,
    presence: plan::Presence,
    condition: &str,
) -> String {
    let ident = || {
        field_camel_ren(
            &field.name,
            rename_of(&field.traits, LANG).as_deref(),
            config,
        )
    };
    match presence {
        plan::Presence::Always => condition.to_string(),
        plan::Presence::String => format!(
            "s.{} !== {} && {condition}",
            ident(),
            cast_string(&field.target, "\"\"")
        ),
        plan::Presence::Bytes => format!("s.{}.length !== 0 && {condition}", ident()),
        plan::Presence::Numeric => {
            let zero = if matches!(field.target, Tref::Prim(Prim::I64 | Prim::U64)) {
                "0n"
            } else {
                "0"
            };
            format!(
                "({err} === undefined || s.{ident} !== {zero}) && {condition}",
                err = err_var(&field.name),
                ident = ident(),
            )
        }
    }
}

/// Declared validation runs last, over every listed field's resolved value:
/// one guard per constraint, collected into a single `violations` throw.
fn validation_guards_block(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    fields: &[&EntryField],
    refs: &mut Vec<Symbol>,
) -> String {
    let en = error_names();
    let mut guards = String::new();
    for field in fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = crate::codegen::entries::field_as_member(field);
        for line in validation::guard_lines(&[member], &TsVal, "s.", config, LANG) {
            let presence = plan::presence_kind(field, entry, module);
            let guard = guard_condition(field, config, presence, &line.condition);
            guards.push_str(&format!(
                "    if ({guard}) {{\n      violations.push({{ field: {field:?}, constraint: {constraint:?}, message: {message:?} }});\n    }}\n",
                field = line.field,
                constraint = line.constraint,
                message = line.message,
            ));
        }
    }
    if guards.is_empty() {
        return String::new();
    }
    refs.push(module_symbol(&en.violation, module));
    refs.push(module_symbol(&en.validation, module));
    format!(
        "    const violations: {violation}[] = [];\n{guards}    if (violations.length > 0) {{\n      throw new {validation}(violations);\n    }}\n",
        violation = en.violation,
        validation = en.validation,
    )
}

/// One `@timeout` field path's declaration and eager-conversion assignment.
/// The async shape resolves this value in `fromSettings` (which owns no
/// `this`), so it lands in a local the private constructor takes as a
/// parameter instead of an eager `this.<field>` write.
struct TimeoutField {
    decl: String,
    assign: String,
    name: String,
}

fn timeout_field(
    entry: &EntryModel<'_>,
    config: &CasingConfig,
    helpers: &mut Helpers,
    is_async: bool,
    vp: &ValuePath<'_>,
    segments: &[String],
) -> TimeoutField {
    let field_name = timeout_field_name(entry, config, segments);
    let decl = format!("  private readonly {field_name}: number;\n");
    // A member of a structured draft may be undefined (the draft starts from
    // an empty object); reading it through its zero keeps this aligned with
    // the constructor's other member reads.
    let expr = if vp.member.is_some() {
        format!(
            "(s.{} ?? {})",
            access(vp, config),
            cast_string(vp.target, "\"\"")
        )
    } else {
        format!("s.{}", access(vp, config))
    };
    helpers.duration_ms = true;
    let fail = config_error(&format!(
        "`{path}: invalid duration ${{JSON.stringify(String({expr}))}}`",
        path = vp.path,
    ));
    let (ok_target, zero_target, predeclare) = if is_async {
        (field_name.clone(), field_name.clone(), true)
    } else {
        (
            format!("this.{field_name}"),
            format!("this.{field_name}"),
            false,
        )
    };
    let mut assign = if predeclare {
        format!("    let {field_name}: number;\n")
    } else {
        String::new()
    };
    let convert = format!(
        "    try {{\n      {ok_target} = durationToMs(String({expr}));\n    }} catch {{\n      {fail}\n    }}\n",
    );
    match presence_guard(entry, vp, &expr) {
        Some(guard) => assign.push_str(&format!(
            "    if ({guard}) {{\n  {convert}    }} else {{\n      {zero_target} = 0;\n    }}\n",
        )),
        None => assign.push_str(&convert),
    }
    TimeoutField {
        decl,
        assign,
        name: field_name,
    }
}

/// Every distinct `@timeout` field path any operation actually declares
/// (usually one, but a multi-op entry could bind more than one) becomes its
/// own private field: construction converts Duration to milliseconds once,
/// eagerly, so a malformed value still fails construction (`ConfigError`)
/// rather than surfacing at the first call that happens to need it.
/// `@retry`/`endpoint`/`@header`/path-template positions all read the typed
/// Settings (or, for retry, the resolved numeric field) directly at the call
/// site (see `op_method`'s `field_expr`), so there is no runtime values map
/// left to freeze into.
struct TimeoutFields {
    decls: String,
    by_path: BTreeMap<String, String>,
    names: Vec<String>,
}

fn timeout_fields_block(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    is_async: bool,
    body: &mut String,
) -> TimeoutFields {
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
    let mut out = TimeoutFields {
        decls: String::new(),
        by_path: BTreeMap::new(),
        names: Vec::new(),
    };
    for vp in entry.value_paths(module) {
        let Some(segments) = timeout_paths.get(&vp.path) else {
            continue;
        };
        let field = timeout_field(entry, config, helpers, is_async, &vp, segments);
        out.decls.push_str(&field.decl);
        body.push_str(&field.assign);
        out.by_path.insert(vp.path.clone(), field.name.clone());
        out.names.push(field.name);
    }
    out
}

/// The frozen client options: the per-operation endpoint resolves from the
/// wire binding, so the options carry only the transport slots bridged off
/// the resolved Settings. The mutually-exclusive transport slots are
/// rejected at construction (as Go does), so a misconfigured client fails to
/// build instead of failing obscurely on its first call. With no wire
/// operation in the module there is no transport and nothing to freeze.
fn options_assign_line(module: &Module, is_async: bool) -> &'static str {
    match (has_wire_ops(module), is_async) {
        (false, true) => "",
        (false, false) => "    this.settings = s;\n",
        (true, true) => "    const options: ClientOptions = { fetch: s.fetch, transport: s.transport, headers: s.headers };\n    assertExclusiveTransport(options);\n",
        (true, false) => "    this.settings = s;\n    this.options = { fetch: s.fetch, transport: s.transport, headers: s.headers };\n    assertExclusiveTransport(this.options);\n",
    }
}

#[allow(clippy::too_many_arguments)]
fn methods_block(
    n: &Names,
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    timeout_field_by_path: &BTreeMap<String, String>,
    refs: &mut Vec<Symbol>,
    helpers: &mut Helpers,
) -> String {
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
            refs,
            &mut helpers.ext_error_types,
        ));
        methods.push('\n');
    }
    methods
}

/// The private static `newSettings`: the shared settings step every
/// construction (and every generated test) starts from.
fn new_settings_text(n: &Names, params: &str, body: &str) -> String {
    format!(
        "  // newSettings resolves every construction value of {client} that no foreign\n\
         \x20 // call reaches, in declared order, then runs the consumed-chain requires\n\
         \x20 // and the declared validation over them.\n\
         \x20 private static newSettings({params}): {settings} {{\n{body}    return s;\n  }}\n\n",
        client = n.client,
        settings = n.settings,
    )
}

/// The `const s: Settings = { ... }` draft every stored field starts from.
fn draft_line(entry: &EntryModel<'_>, n: &Names, module: &Module, config: &CasingConfig) -> String {
    let zeros: String = entry
        .stored(module)
        .iter()
        .map(|f| {
            format!(
                "{}: {}, ",
                field_camel_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
                zero_value(&f.target, module)
            )
        })
        .collect();
    let init = if has_wire_ops(module) {
        format!("{{ {zeros}headers: {{}} }}")
    } else {
        let trimmed = zeros.trim_end_matches(", ");
        if trimmed.is_empty() {
            "{}".to_string()
        } else {
            format!("{{ {trimmed} }}")
        }
    };
    format!("    const s: {settings} = {init};\n", settings = n.settings)
}

/// The full class text, assembled from every block above, in one of two
/// shapes: a plain synchronous `constructor`, or (see [`class_decl`]'s own
/// doc) a `private constructor` fed by a `static async create` factory.
struct ClassParts<'a> {
    doc: String,
    names: &'a Names,
    client: &'a str,
    entry_name: &'a str,
    settings: &'a str,
    params: String,
    pass: String,
    timeout_field_decls: String,
    /// The `newSettings` body (the draft, the shared step's resolution,
    /// requires and validation).
    settings_body: String,
    /// The foreign constructions and whatever depends on them, in
    /// resolution order (empty on the sync shape).
    tail_body: String,
    /// The timeouts and the frozen options.
    finish_body: String,
    /// `forTest`'s static factory: the real construction path plus the
    /// transport seam a generated test constructs through. Empty when the
    /// entry declares no test (nothing composes it) or has no wire operation
    /// (nothing on `options` to swap).
    for_test: String,
    methods: String,
    wire: bool,
}

const CTOR_DOC: &str =
    "// constructor takes the @arg values positionally and the @with values as a\n\
     // config object; construction resolves the declared sources, validates,\n\
     // and freezes the runtime options.\n";

fn options_field(wire: bool) -> &'static str {
    if wire {
        "  private readonly options: ClientOptions;\n"
    } else {
        ""
    }
}

/// `forTest`'s static factory: the real construction path (`create`, or the
/// constructor on the sync shape) runs first, then the canonical transport
/// replaces whatever construction resolved, so a test answers canonically
/// without a server. `ClientOptions` spells its slots readonly, so the swap
/// goes through a mutable view of the frozen options object (the object
/// itself is a plain literal). Empty when the entry declares no test
/// (nothing composes it) or has no wire operation (nothing on `options` to
/// swap).
#[allow(clippy::too_many_arguments)]
fn for_test_block(
    n: &Names,
    params: &str,
    pass: &str,
    is_async: bool,
    has_tests: bool,
    wire: bool,
    refs: &mut Vec<Symbol>,
) -> String {
    if !has_tests || !wire {
        return String::new();
    }
    refs.push(support_symbol("HttpTransport"));
    let sig_params = if params.is_empty() {
        String::new()
    } else {
        format!(", {params}")
    };
    let (async_kw, construct, await_kw) = if is_async {
        (
            "async ",
            format!("await {}.create({pass})", n.client),
            "await ",
        )
    } else {
        ("", format!("new {}({pass})", n.client), "")
    };
    format!(
        "  // forTest is the constructor plus the transport seam the generated\n\
         \x20 // tests construct through: the real construction path runs first, then\n\
         \x20 // the canonical transport wins over anything bespoke.\n\
         \x20 static {async_kw}forTest(seam: {{ transport: HttpTransport }}{sig_params}): {promise_open}{client}{promise_close} {{\n\
         \x20   const client = {await_kw}{construct};\n\
         \x20   const options = client.options as {{ transport?: HttpTransport; fetch?: typeof fetch }};\n\
         \x20   options.transport = seam.transport;\n\
         \x20   options.fetch = undefined;\n\
         \x20   return client;\n\
         \x20 }}\n\n",
        client = n.client,
        promise_open = if is_async { "Promise<" } else { "" },
        promise_close = if is_async { ">" } else { "" },
    )
}

fn sync_class_text(p: &ClassParts<'_>) -> String {
    format!(
        "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
         {CTOR_DOC}\
         export class {client} {{\n\
         \x20 private readonly settings: {settings};\n\
         {options}\
         {timeout_field_decls}\
         \x20 constructor({params}) {{\n    const s = {client}.newSettings({pass});\n{finish}  }}\n\n\
         {new_settings}{for_test}{methods}}}",
        doc = p.doc,
        client = p.client,
        entry_name = p.entry_name,
        settings = p.settings,
        options = options_field(p.wire),
        timeout_field_decls = p.timeout_field_decls,
        params = p.params,
        pass = p.pass,
        finish = p.finish_body,
        new_settings = new_settings_text(p.names, &p.params, &p.settings_body),
        for_test = p.for_test,
        methods = p.methods,
    )
}

/// The entry resolves at least one field through a foreign call: a
/// TypeScript constructor cannot itself be async, so construction goes
/// through the static `create` factory instead of `new {client}(...)`; the
/// same `.tono` source keeps Go blocking. `fromSettings` is the last step,
/// over assembled settings: the generated tests compose it with their
/// fakes.
fn async_class_text(p: &ClassParts<'_>, timeout_field_names: &[String]) -> String {
    let ctor_params: String = std::iter::once(format!("settings: {}", p.settings))
        .chain(p.wire.then(|| "options: ClientOptions".to_string()))
        .chain(timeout_field_names.iter().map(|f| format!("{f}: number")))
        .collect::<Vec<_>>()
        .join(", ");
    let ctor_body: String = std::iter::once("    this.settings = settings;\n".to_string())
        .chain(p.wire.then(|| "    this.options = options;\n".to_string()))
        .chain(
            timeout_field_names
                .iter()
                .map(|f| format!("    this.{f} = {f};\n")),
        )
        .collect();
    let create_return = std::iter::once("s".to_string())
        .chain(p.wire.then(|| "options".to_string()))
        .chain(timeout_field_names.iter().cloned())
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
         {CTOR_DOC}\
         export class {client} {{\n\
         \x20 private readonly settings: {settings};\n\
         {options}\
         {timeout_field_decls}\
         \x20 private constructor({ctor_params}) {{\n{ctor_body}  }}\n\n\
         \x20 static async create({params}): Promise<{client}> {{\n    const s = {client}.newSettings({pass});\n{tail}    return {client}.fromSettings(s);\n  }}\n\n\
         {new_settings}\
         \x20 // fromSettings builds {client} over assembled settings: the runtime values\n\
         \x20 // freeze here, after every construction value resolved.\n\
         \x20 private static fromSettings(s: {settings}): {client} {{\n{finish}    return new {client}({create_return});\n  }}\n\n\
         {for_test}{methods}}}",
        doc = p.doc,
        client = p.client,
        entry_name = p.entry_name,
        settings = p.settings,
        options = options_field(p.wire),
        timeout_field_decls = p.timeout_field_decls,
        params = p.params,
        pass = p.pass,
        tail = p.tail_body,
        new_settings = new_settings_text(p.names, &p.params, &p.settings_body),
        finish = p.finish_body,
        for_test = p.for_test,
        methods = p.methods,
    )
}

/// The class per entry: construction (resolution, bridge, validation, frozen
/// options) plus one async method per operation.
///
/// TypeScript's own identity is "throws, and may return a Promise": nothing
/// in the IR marks a given extern call sync or async, and the compiler
/// cannot know statically whether a third-party function returns a Promise,
/// so every resolver is awaited unconditionally. An entry with at least one
/// foreign construction therefore always resolves through an async path
/// here, while Go keeps its blocking constructor for the identical `.tono`
/// source: sync vs async is a target's own idiom, lowered per language the
/// same way at every site it comes up.
#[allow(clippy::too_many_arguments)]
pub(super) fn class_decl(
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
    let split = entry.construction_split(module);
    let is_async = !split.tail.is_empty();
    let wire = has_wire_ops(module);
    let mut refs = base_refs(entry, module);
    let (params, pass) = ctor_params(entry, n, module);

    // newSettings: the draft and the shared step.
    let mut settings_body = draft_line(entry, n, module, config);
    let (steps, mut resolve_fns) = resolution_steps(
        entry,
        module,
        config,
        helpers,
        multi,
        n,
        &split.settings,
        &mut refs,
    );
    settings_body.push_str(&steps);

    // create: the foreign constructions and what depends on them.
    let tail_fields: Vec<&EntryField> = split.tail.iter().map(|s| s.field()).collect();
    let (tail_body, more) = resolution_steps(
        entry,
        module,
        config,
        helpers,
        multi,
        n,
        &tail_fields,
        &mut refs,
    );
    resolve_fns.extend(more);
    for step in &split.tail {
        if matches!(
            step,
            crate::codegen::entries::TailStep::Call(_)
                | crate::codegen::entries::TailStep::HandleCall(_)
        ) {
            refs.push(module_symbol(
                &super::ext_resolver::resolver_name(n, step.field()),
                module,
            ));
        }
    }
    // fromSettings / the constructor's own tail: timeouts, frozen options.
    let mut finish_body = String::new();
    let timeouts = timeout_fields_block(entry, module, config, helpers, is_async, &mut finish_body);
    finish_body.push_str(options_assign_line(module, is_async));
    let methods = methods_block(
        n,
        entry,
        module,
        config,
        bound,
        &timeouts.by_path,
        &mut refs,
        helpers,
    );
    let for_test = for_test_block(
        n,
        &params.join(", "),
        &pass.join(", "),
        is_async,
        has_tests,
        wire,
        &mut refs,
    );

    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let parts = ClassParts {
        doc,
        names: n,
        client: &n.client,
        entry_name: entry.name,
        settings: &n.settings,
        params: params.join(", "),
        pass: pass.join(", "),
        timeout_field_decls: timeouts.decls,
        settings_body,
        tail_body,
        finish_body,
        for_test,
        methods,
        wire,
    };
    let text = if is_async {
        async_class_text(&parts, &timeouts.names)
    } else {
        sync_class_text(&parts)
    };

    // Any construction-time failure (a bad env value, a malformed blob, an
    // unmatched select, an unresolved consumed chain) throws ConfigError, so
    // the class file imports that category when the body emits one.
    if text.contains(&format!("new {}(", en.config)) {
        refs.push(module_symbol(&en.config, module));
    }
    refs.append(&mut helpers.ext_refs);
    resolve_fns.push(Decl::raw_with(text, refs));
    resolve_fns
}

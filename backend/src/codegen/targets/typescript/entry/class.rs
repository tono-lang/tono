//! The class per entry: constructor (resolution, bridge, validation, frozen
//! options) plus one async method per operation. Split out of `entry/mod.rs`
//! to stay under this repo's per-file line ceiling.

use super::*;

/// The class per entry: constructor (resolution, bridge, validation, frozen
/// options) plus one async method per operation.
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
    // TypeScript's own identity is "lança e pode devolver Promise"
    // (RFC-0023): nothing in the IR marks a given extern call sync or
    // async, and the compiler cannot know statically whether a
    // third-party function returns a Promise, so `ext_call::call_assign`
    // awaits every one unconditionally. An entry with at least one such
    // field therefore always resolves through an async path here; ADR-0010
    // applied to construction means Go stays blocking for the identical
    // `.tono` source.
    let is_async = entry.fields.iter().any(|f| f.call.is_some());
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
                zero_value(&f.target, module)
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
    // Emission order, so `create` (the async path) can pass every timeout
    // local to the private constructor positionally.
    let mut timeout_field_names: Vec<String> = Vec::new();
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
        // The async path resolves this value in `create` (which owns no
        // `this`), so it lands in a local the private constructor takes as
        // a parameter instead of an eager `this.<field>` write.
        let (ok_target, zero_target) = if is_async {
            body.push_str(&format!("    let {field_name}: number;\n"));
            (field_name.clone(), field_name.clone())
        } else {
            (format!("this.{field_name}"), format!("this.{field_name}"))
        };
        let assign = format!(
            "    try {{\n      {ok_target} = durationToMs(String({expr}));\n    }} catch {{\n      {fail}\n    }}\n",
        );
        match presence_guard(entry, &vp, &expr) {
            Some(guard) => body.push_str(&format!(
                "    if ({guard}) {{\n  {assign}    }} else {{\n      {zero_target} = 0;\n    }}\n",
            )),
            None => body.push_str(&assign),
        }
        timeout_field_by_path.insert(vp.path.clone(), field_name.clone());
        timeout_field_names.push(field_name);
    }

    // The frozen client options: the per-operation endpoint resolves from the
    // wire binding, so the options carry only the transport slots bridged off
    // the resolved Settings. The mutually-exclusive transport slots are
    // rejected at construction (as Go does in New), so a misconfigured
    // client fails to build instead of failing obscurely on its first call.
    // The async path resolves both in `create`, as locals passed to the
    // private constructor, for the same reason as the timeout fields above.
    if is_async {
        body.push_str(
            "    const options: ClientOptions = { fetch: s.fetch, transport: s.transport, headers: s.headers };\n    assertExclusiveTransport(options);\n",
        );
    } else {
        body.push_str(
            "    this.settings = s;\n    this.options = { fetch: s.fetch, transport: s.transport, headers: s.headers };\n    assertExclusiveTransport(this.options);\n",
        );
    }

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
        let (async_kw, construct, await_kw) = if is_async {
            (
                "async ",
                format!("await {}.create({})", n.client, pass.join(", ")),
                "await ",
            )
        } else {
            ("", format!("new {}({})", n.client, pass.join(", ")), "")
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
    } else {
        String::new()
    };

    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let ctor_doc = "// constructor takes the @arg values positionally and the @with values as a\n\
         // config object; construction resolves the declared sources, runs the\n\
         // client_init bridge, validates, and freezes the runtime options.\n";
    let text = if is_async {
        // The entry resolves at least one field through an async external
        // call (RFC-0023): a TypeScript constructor cannot itself be async,
        // so construction goes through this static factory instead of
        // `new {client}(...)`; the same `.tono` source keeps Go blocking
        // (ADR-0010).
        let ctor_params: String = std::iter::once(format!("settings: {}", n.settings))
            .chain(std::iter::once("options: ClientOptions".to_string()))
            .chain(timeout_field_names.iter().map(|f| format!("{f}: number")))
            .collect::<Vec<_>>()
            .join(", ");
        let ctor_body: String = std::iter::once(
            "    this.settings = settings;\n    this.options = options;\n".to_string(),
        )
        .chain(
            timeout_field_names
                .iter()
                .map(|f| format!("    this.{f} = {f};\n")),
        )
        .collect();
        let create_return = std::iter::once("s".to_string())
            .chain(std::iter::once("options".to_string()))
            .chain(timeout_field_names.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
             {ctor_doc}\
             export class {client} {{\n\
             \x20 private readonly settings: {settings};\n\
             \x20 private readonly options: ClientOptions;\n\
             {timeout_field_decls}\
             {hooks_field}\
             \x20 private constructor({ctor_params}) {{\n{ctor_body}  }}\n\n\
             \x20 static async create({params}): Promise<{client}> {{\n{body}    return new {client}({create_return});\n  }}\n\n\
             {for_test}{methods}}}",
            client = n.client,
            entry_name = entry.name,
            settings = n.settings,
            params = params.join(", "),
        )
    } else {
        format!(
            "{doc}// {client} is the generated SDK client the {entry_name} entry declares. The\n\
             {ctor_doc}\
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
        )
    };
    // Any construction-time failure (a bad env value, a malformed blob, an
    // unmatched select, an unresolved consumed chain) throws ConfigError, so
    // the class file imports that category when the body emits one.
    if text.contains(&format!("new {}(", en.config)) {
        refs.push(module_symbol(&en.config, module));
    }
    // An extern-call leaf (RFC-0023) has no other way back to this Decl's
    // own refs than the shared `Helpers` sink `plan::Emitter::call_assign`
    // writes into (see `ext_call::call_assign`).
    refs.append(&mut helpers.ext_refs);
    resolve_fns.push(Decl::raw_with(text, refs));
    resolve_fns
}

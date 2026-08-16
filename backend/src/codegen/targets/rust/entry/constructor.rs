//! The generated constructor (a builder's `build` or a plain `new`): source
//! resolution in dependency order, the consumed-chain requires, declared
//! validation, the eager `@timeout` millisecond conversion, and the transport
//! slots frozen into `ClientOptions`. Split out of `mod.rs` to stay under this
//! repo's per-file line ceiling; `construction_decls` is `mod.rs`'s only
//! caller.

use std::collections::BTreeMap;

use super::*;

/// Every distinct `@timeout` field path any operation declares, keyed by its
/// dotted spelling, paired with the private client field its converted
/// millisecond value lands in. The constructor converts once, eagerly, so a
/// malformed duration still fails construction (`ConfigError`) rather than
/// surfacing at the first call that happens to need it.
pub(super) fn timeout_fields(entry: &EntryModel<'_>) -> BTreeMap<String, String> {
    entry
        .operations
        .iter()
        .filter_map(|op| match &op.kind {
            ShapeKind::Operation { wire: Some(w), .. } => w
                .timeout
                .as_ref()
                .map(|p| (p.join("."), format!("{}_ms", snake(&p.join("_"))))),
            _ => None,
        })
        .collect()
}

/// Whether any operation declares `@retry`, which is what puts the timing
/// seam (the swappable sleep/random pair) on the generated client at all.
fn has_retry(entry: &EntryModel<'_>) -> bool {
    entry.operations.iter().any(
        |op| matches!(&op.kind, ShapeKind::Operation { wire: Some(w), .. } if w.retry.is_some()),
    )
}

/// The declared-validation guard access-prefix syntax the entry construction
/// shares with structure validators.
struct RustVal;
impl validation::ValSyntax for RustVal {
    fn length(&self, access: &str, measure: validation::Measure) -> String {
        match measure {
            validation::Measure::Chars => format!("{access}.chars().count()"),
            validation::Measure::Elements | validation::Measure::Bytes => format!("{access}.len()"),
        }
    }
    fn wide_suffix(&self) -> &str {
        ""
    }
}

/// The `Client` struct, its builder (or plain constructor) and their bodies,
/// and the `impl Client` block carrying one method per operation.
///
/// With `test_seam`, the whole construction body moves into a `pub(crate)`
/// variant taking an `Option<Transport>`, and the public entry point
/// delegates with `None`: a generated test runs the real construction path
/// (resolution, validation) and only the transport is swapped, so the test
/// sees exactly the request the SDK would send. `pub(crate)` suffices because
/// the generated test file is a `#[cfg(test)]` module of the same crate.
#[allow(clippy::too_many_arguments)]
pub(super) fn construction_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
    test_seam: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    let mut refs = vec![];
    let mut resolve_fns: Vec<Decl> = Vec::new();
    let with_fields = entry.with_fields();
    let args = entry.args();
    let timeouts = timeout_fields(entry);
    let seam = has_retry(entry);
    // A call-sourced field's emitted call is always awaited (`resolve_call`),
    // so construction itself becomes async whenever the entry has one,
    // mirroring `impl_op`'s own `is_async` gate for op methods: Rust is an
    // async-lowering target, so a construction that depends on an external
    // call takes the language's idiomatic async form.
    let is_async = entry.declared().iter().any(|f| f.call.is_some());
    let effect = if is_async { "async " } else { "" };
    let awaited = if is_async { ".await" } else { "" };

    // The client struct itself: the resolved settings, the frozen transport
    // options, one private field per distinct `@timeout` path (converted to
    // milliseconds eagerly; see `resolution_body`), and, when any operation
    // retries, the crate-visible timing seam the parity harness pins.
    let doc = doc_of(&entry.shape.traits)
        .map(|d| crate::codegen::doc::rustdoc(&d, ""))
        .unwrap_or_default();
    let timeout_field_decls: String = timeouts
        .values()
        .map(|field| format!("    {field}: f64,\n"))
        .collect();
    let seam_field_decls = if seam {
        "    pub(crate) sleep: SleepFn,\n    pub(crate) random: RandomFn,\n"
    } else {
        ""
    };
    let mut client_refs = vec![support_symbol("ClientOptions")];
    if seam {
        client_refs.push(shared_symbol("SleepFn"));
        client_refs.push(shared_symbol("RandomFn"));
    }
    decls.push(Decl::raw_with(
        format!(
            // An entry whose ops read no settings field directly (every wire
            // position a literal) never reads `settings` back after
            // construction; a bespoke `ext impl` op does. `dead_code` is
            // silenced per entry rather than omitting the field, so its shape
            // stays uniform across entries regardless of which ops happen to
            // need it.
            "{doc}pub struct {client} {{\n    #[allow(dead_code)]\n    settings: {settings},\n    options: ClientOptions,\n{timeout_field_decls}{seam_field_decls}}}",
            client = n.client,
            settings = n.settings,
        ),
        client_refs,
    ));

    let ctx = BodyCtx {
        timeouts: &timeouts,
        seam,
        test_seam,
    };
    let body = resolution_body(
        entry,
        n,
        module,
        config,
        helpers,
        multi,
        "self.",
        &ctx,
        &mut refs,
        &mut resolve_fns,
    );
    let plain_body = resolution_body(
        entry,
        n,
        module,
        config,
        helpers,
        multi,
        "",
        &ctx,
        &mut refs,
        &mut resolve_fns,
    );
    decls.extend(resolve_fns);

    if with_fields.is_empty() {
        // No @with fields: a builder would have nothing to configure, so the
        // entry point is a plain associated function.
        let params: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "{}: {}",
                    arg_snake(&f.name, &f.traits, LANG),
                    rust_type(&f.target)
                )
            })
            .collect();
        let doc = format!(
            "    /// Constructs {client}: the declared sources resolve top-down, then\n    /// the declared validation.\n",
            client = n.client,
        );
        let constructors = if test_seam {
            let pass: Vec<String> = args
                .iter()
                .map(|f| arg_snake(&f.name, &f.traits, LANG))
                .collect();
            let comma = if params.is_empty() { "" } else { ", " };
            refs.push(support_symbol("HttpTransport"));
            format!(
                "{doc}    pub {effect}fn new({params}) -> Result<Self, TonoError> {{\n        Self::new_with_transport(None{comma}{pass}){awaited}\n    }}\n\n    /// `new` plus the transport seam the generated tests construct through:\n    /// a `Some` transport replaces whatever construction resolved, so a test\n    /// answers canonically without a server.\n    pub(crate) {effect}fn new_with_transport(transport: Option<HttpTransport>{comma}{params}) -> Result<Self, TonoError> {{\n{body}\n    }}\n",
                params = params.join(", "),
                pass = pass.join(", "),
                body = indent(&plain_body, 2),
            )
        } else {
            format!(
                "{doc}    pub {effect}fn new({params}) -> Result<Self, TonoError> {{\n{body}\n    }}\n",
                params = params.join(", "),
                body = indent(&plain_body, 2),
            )
        };
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n{constructors}{methods}}}",
                client = n.client,
                methods = op_methods(entry, n, module, config, bound, &timeouts, &mut refs),
            ),
            refs,
        ));
    } else {
        // A builder: the @arg fields are captured positionally at
        // `builder()`, the @with fields default to unset, and `build`
        // consumes the builder to run the same resolution over `self.*`.
        let mut builder_fields: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "    {}: {},",
                    arg_snake(&f.name, &f.traits, LANG),
                    rust_type(&f.target)
                )
            })
            .collect();
        builder_fields.extend(with_fields.iter().map(|f| {
            format!(
                "    {}: Option<{}>,",
                arg_snake(&f.name, &f.traits, LANG),
                rust_type(&f.target)
            )
        }));
        decls.push(Decl::raw(format!(
            "pub struct {builder} {{\n{fields}\n}}",
            builder = n.builder,
            fields = builder_fields.join("\n"),
        )));

        let with_methods: String = with_fields
            .iter()
            .map(|f| {
                let display = rename_of(&f.traits, LANG).unwrap_or_else(|| f.name.clone());
                let fn_name = snake(&format!("with_{}", companion_name(entry.name, &display, multi)));
                let member = arg_snake(&f.name, &f.traits, LANG);
                let doc = field_doc(&f.traits, "    ");
                format!(
                    "{doc}    pub fn {fn_name}(mut self, v: {ty}) -> Self {{\n        self.{member} = Some(v);\n        self\n    }}\n",
                    ty = rust_type(&f.target),
                )
            })
            .collect();

        let build_doc = "    /// Resolves the declared sources top-down, then the declared\n    /// validation.\n";
        let build_fns = if test_seam {
            refs.push(support_symbol("HttpTransport"));
            format!(
                "{build_doc}    pub {effect}fn build(self) -> Result<{client}, TonoError> {{\n        self.build_with_transport(None){awaited}\n    }}\n\n    /// `build` plus the transport seam the generated tests construct through:\n    /// a `Some` transport replaces whatever construction resolved, so a\n    /// test answers canonically without a server.\n    pub(crate) {effect}fn build_with_transport(self, transport: Option<HttpTransport>) -> Result<{client}, TonoError> {{\n{body}\n    }}\n",
                client = n.client,
                body = indent(&body, 2),
            )
        } else {
            format!(
                "{build_doc}    pub {effect}fn build(self) -> Result<{client}, TonoError> {{\n{body}\n    }}\n",
                client = n.client,
                body = indent(&body, 2),
            )
        };
        decls.push(Decl::raw_with(
            format!(
                "impl {builder} {{\n{with_methods}\n{build_fns}}}",
                builder = n.builder,
            ),
            refs.clone(),
        ));

        let params: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "{}: {}",
                    arg_snake(&f.name, &f.traits, LANG),
                    rust_type(&f.target)
                )
            })
            .collect();
        let field_inits: Vec<String> = args
            .iter()
            .map(|f| arg_snake(&f.name, &f.traits, LANG))
            .chain(
                with_fields
                    .iter()
                    .map(|f| format!("{}: None", arg_snake(&f.name, &f.traits, LANG))),
            )
            .collect();
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n    /// The entry point of the builder: the @arg values positionally,\n    /// @with values configured through `.with_*` before `.build()`.\n    pub fn builder({params}) -> {builder} {{\n        {builder} {{ {inits} }}\n    }}\n{methods}}}",
                client = n.client,
                builder = n.builder,
                params = params.join(", "),
                inits = field_inits.join(", "),
                methods = op_methods(entry, n, module, config, bound, &timeouts, &mut refs),
            ),
            refs,
        ));
    }

    decls
}

/// What the resolution body needs beyond the entry itself: the `@timeout`
/// millisecond fields to convert into, whether the timing seam rides the
/// client, and whether the test transport seam parameter exists.
struct BodyCtx<'a> {
    timeouts: &'a BTreeMap<String, String>,
    seam: bool,
    test_seam: bool,
}

/// The full resolution body shared by the builder's `build` and the
/// plain-constructor `new`: zero the draft, resolve every field, run the
/// consumed-chain requires and the declared validation, convert each
/// `@timeout` path to milliseconds eagerly (a malformed duration fails
/// construction, not the first call), and freeze the transport slots into
/// `ClientOptions`. `arg_prefix` is `"self."` when
/// reading an `@arg` value off a builder (`build(self)` consumes it) or
/// empty when reading it off a bare function parameter (`new`'s own
/// arguments) — the only place the two entry points differ. With
/// `test_seam`, a `transport` parameter override lands right before the
/// options freeze, so the test transport wins over anything bespoke set.
#[allow(clippy::too_many_arguments)]
fn resolution_body(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    multi: bool,
    arg_prefix: &'static str,
    ctx: &BodyCtx<'_>,
    refs: &mut Vec<Symbol>,
    resolve_fns: &mut Vec<Decl>,
) -> String {
    let mut body = String::new();

    let zeros: String = entry
        .declared()
        .iter()
        .map(|f| {
            format!(
                "{}: {}, ",
                field_snake_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
                zero_value(&f.target, module, config),
            )
        })
        .collect();
    body.push_str(&format!(
        "let mut s = {settings} {{\n    {zeros}\n    #[cfg(feature = \"reqwest\")]\n    client: None,\n    transport: None,\n    headers: std::collections::HashMap::new(),\n}};\n",
        settings = n.settings,
        zeros = zeros.trim_end(),
    ));

    {
        let mut r = resolve::Resolver {
            entry,
            module,
            config,
            helpers,
            arg_prefix,
            body: &mut body,
            refs,
            resolve_fns,
            multi,
        };
        let fields = plan::emit_fields(entry, module, &mut r, 1);
        if !fields.is_empty() {
            plan::push_gap(r.body);
            r.body.push_str(&fields);
        }
    }

    {
        let mut r = resolve::Resolver {
            entry,
            module,
            config,
            helpers,
            arg_prefix,
            body: &mut body,
            refs,
            resolve_fns,
            multi,
        };
        let requires = plan::build_requires(entry, module, &mut r);
        let text = plan::render(&requires, 0, &r);
        if !text.is_empty() {
            plan::push_gap(r.body);
            r.body.push_str(&text);
        }
    }

    let mut guards = String::new();
    for field in &entry.fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = crate::codegen::entries::field_as_member(field);
        for line in validation::guard_lines(&[member], &RustVal, "s.", config, LANG) {
            let ident = field_snake_ren(
                &field.name,
                rename_of(&field.traits, LANG).as_deref(),
                config,
            );
            let guard = match plan::presence_kind(field, entry, module) {
                plan::Presence::Always => line.condition.clone(),
                plan::Presence::String => format!(
                    "s.{ident} != {} && {}",
                    zero_value(&field.target, module, config),
                    line.condition
                ),
                plan::Presence::Bytes => format!("!s.{ident}.is_empty() && {}", line.condition),
                plan::Presence::Numeric => format!(
                    "({err}.is_none() || s.{ident} != {}) && {}",
                    numeric_zero(&field.target),
                    line.condition,
                    err = err_var(&field.name),
                ),
            };
            guards.push_str(&format!(
                "if {guard} {{\n    violations.push(Violation {{ field: {field:?}.to_string(), constraint: {constraint:?}.to_string(), message: {message:?}.to_string() }});\n}}\n",
                field = line.field,
                constraint = line.constraint,
                message = line.message,
            ));
        }
    }
    if !guards.is_empty() {
        plan::push_gap(&mut body);
        body.push_str(&format!(
            "let mut violations: Vec<Violation> = Vec::new();\n{guards}if !violations.is_empty() {{\n    return Err(TonoError::Validation(ValidationError {{ violations }}));\n}}\n",
        ));
    }

    body.push_str(&discard_unread_errs(&body, entry));

    // Each `@timeout` path converts to milliseconds now, so a malformed
    // duration fails construction (`ConfigError`) rather than surfacing at
    // the first call that happens to need it. `@retry`/endpoint/`@header`/
    // path-template positions all read the typed Settings directly at the
    // call site (see `transport::op_call`), so there is no runtime value bag
    // left to freeze.
    for (path, field_name) in ctx.timeouts {
        let Some(vp) = entry
            .value_paths(module)
            .into_iter()
            .find(|vp| vp.path == *path)
        else {
            continue;
        };
        let expr = format!(
            "s.{}",
            crate::codegen::entries::value_path_access(&vp, config, LANG)
        );
        let convert = if let Tref::Prim(Prim::Duration) = vp.target {
            refs.push(shared_symbol("parse_duration_ms"));
            format!(
                "match {parse}(&{expr}.0) {{\n    Ok(ms) => ms,\n    Err(_) => {{\n        return Err(TonoError::Config(ConfigError {{ message: format!(\"{path}: invalid duration {{:?}}\", {expr}.0) }}));\n    }}\n}}",
                parse = shared_slot("parse_duration_ms"),
            )
        } else {
            format!("{expr} as f64")
        };
        match checks::presence_guard(entry, &vp, &expr, module, config) {
            Some(guard) => body.push_str(&format!(
                "let {field_name} = if {guard} {{\n{convert_in}}} else {{\n    0.0\n}};\n",
                convert_in = indent(&convert, 1),
            )),
            None => body.push_str(&format!("let {field_name} = {convert};\n")),
        }
    }

    if ctx.test_seam {
        body.push_str(
            "if let Some(t) = transport {\n    s.transport = Some(t);\n    #[cfg(feature = \"reqwest\")]\n    {\n        s.client = None;\n    }\n}\n",
        );
    }
    refs.push(support_symbol("ClientOptions"));
    refs.push(shared_symbol("check_transport"));
    body.push_str(
        "let options = ClientOptions {\n    #[cfg(feature = \"reqwest\")]\n    client: s.client.clone(),\n    transport: s.transport.clone(),\n    headers: s.headers.clone(),\n};\nif let Err(message) = check_transport(&options) {\n    return Err(TonoError::Config(ConfigError { message }));\n}\n",
    );
    let mut inits = String::from("settings: s, options");
    for field_name in ctx.timeouts.values() {
        inits.push_str(&format!(", {field_name}"));
    }
    if ctx.seam {
        refs.push(shared_symbol("default_sleep"));
        refs.push(shared_symbol("default_random"));
        inits.push_str(", sleep: default_sleep(), random: default_random()");
    }
    body.push_str(&format!("Ok({client} {{ {inits} }})", client = n.client,));

    body
}

/// Discard statements for the error vars nothing reads (mirrors Go's
/// `discard_unread_errs`): every declared source chain records its failure in
/// an error var, but only a field something consumes gets a check reading it
/// back. Rather than predicting consumption ahead of the shared plan's own
/// choice, the error is kept and explicitly discarded so an error var with no
/// consumer is never merely an unused-but-harmless local (idiomatic hygiene
/// for the generated SDK, not a compile requirement of this crate).
fn discard_unread_errs(body: &str, entry: &EntryModel<'_>) -> String {
    entry
        .fields
        .iter()
        .map(|f| err_var(&f.name))
        .filter(|err| {
            plan::is_written_never_read(
                body,
                err,
                |prefix| prefix.ends_with("let") || prefix.ends_with("mut"),
                None,
            )
        })
        .map(|err| format!("let _ = &{err};\n"))
        .collect()
}

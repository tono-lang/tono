//! The generated constructor (a builder's `build` or a plain `new`): source
//! resolution in dependency order, the `client_init` bridge, the
//! consumed-chain requires, declared validation, and the frozen runtime
//! values. Split out of `mod.rs` to stay under this repo's per-file line
//! ceiling; `construction_decls` is `mod.rs`'s only caller.

use super::*;

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
#[allow(clippy::too_many_arguments)]
pub(super) fn construction_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    let mut refs = vec![];
    let with_fields = entry.with_fields();
    let args = entry.args();

    // The client struct itself.
    let doc = doc_of(&entry.shape.traits)
        .map(|d| crate::codegen::doc::rustdoc(&d, ""))
        .unwrap_or_default();
    decls.push(Decl::raw(format!(
        // A purely declarative entry (every op's descriptor ref position
        // resolves through the frozen runtime `values`, not a direct field
        // read) never reads `settings` back after construction; a bespoke
        // `ext impl` op or lifecycle hook does. `dead_code` is silenced per
        // entry rather than omitting the field, so its shape stays uniform
        // across entries regardless of which ops happen to need it.
        "{doc}pub struct {client} {{\n    #[allow(dead_code)]\n    settings: {settings},\n    runtime: std::sync::Arc<tono_http_runtime::Runtime>,\n    hooks: Option<tono_http_runtime::Hooks>,\n}}",
        client = n.client,
        settings = n.settings,
    )));

    let body = resolution_body(
        entry, n, module, config, bound, helpers, multi, "self.", &mut refs,
    );
    let plain_body = resolution_body(
        entry, n, module, config, bound, helpers, multi, "", &mut refs,
    );

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
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n    /// Constructs {client}: the declared sources resolve top-down,\n    /// client_init runs on top (bespoke wins), then the declared validation.\n    pub fn new({params}) -> Result<Self, TonoError> {{\n{body}\n    }}\n{methods}}}",
                client = n.client,
                params = params.join(", "),
                body = indent(&plain_body, 2),
                methods = op_methods(entry, n, module, config, bound, &mut refs),
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

        decls.push(Decl::raw_with(
            format!(
                "impl {builder} {{\n{with_methods}\n    /// Resolves the declared sources top-down, runs client_init on top\n    /// (bespoke wins), then the declared validation.\n    pub fn build(self) -> Result<{client}, TonoError> {{\n{body}\n    }}\n}}",
                builder = n.builder,
                client = n.client,
                body = indent(&body, 2),
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
                methods = op_methods(entry, n, module, config, bound, &mut refs),
            ),
            refs,
        ));
    }

    decls
}

/// The full resolution body shared by the builder's `build` and the
/// plain-constructor `new`: zero the draft, resolve every field, bridge
/// `client_init`, run the consumed-chain requires and the declared
/// validation, freeze the resolved values, and build the runtime. `arg_prefix`
/// is `"self."` when reading an `@arg` value off a builder (`build(self)`
/// consumes it) or empty when reading it off a bare function parameter
/// (`new`'s own arguments) — the only place the two entry points differ.
#[allow(clippy::too_many_arguments)]
fn resolution_body(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
    arg_prefix: &'static str,
    refs: &mut Vec<Symbol>,
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
        "let mut s = {settings} {{ {zeros}client: None, transport: None, headers: std::collections::HashMap::new() }};\n",
        settings = n.settings,
    ));

    {
        let mut r = resolve::Resolver {
            entry,
            module,
            config,
            helpers,
            arg_prefix,
            body: &mut body,
        };
        let fields = plan::emit_fields(entry, module, &mut r);
        r.body.push_str(&fields);
    }

    if !multi {
        if let Some(b) = hook_binding(bound, "client_init") {
            refs.push(Symbol::imported(b.symbol, use_path(b.module), b.symbol));
            body.push_str(&format!(
                "if let Err(e) = {sym}(&mut s) {{\n    return Err(match e.downcast::<TonoError>() {{\n        Ok(declared) => *declared,\n        Err(other) => TonoError::Contract(ContractError {{ contract_name: \"client_init\".to_string(), cause: other }}),\n    }});\n}}\n",
                sym = b.symbol,
            ));
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
        };
        let requires = plan::build_requires(entry, module, &mut r);
        let text = plan::render(&requires, 0, &r);
        r.body.push_str(&text);
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
                    "({why} == \"\" || s.{ident} != {}) && {}",
                    numeric_zero(&field.target),
                    line.condition,
                    why = why_var(&field.name),
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
        body.push_str(&format!(
            "let mut violations: Vec<Violation> = Vec::new();\n{guards}if !violations.is_empty() {{\n    return Err(TonoError::Validation(ValidationError {{ violations }}));\n}}\n",
        ));
    }

    body.push_str(&discard_unread_whys(&body, entry));

    body.push_str("let mut values = serde_json::Map::new();\n");
    for vp in entry.value_paths(module) {
        let scalar_ref = ref_is_enum(vp.target, module);
        let Some(expr) = checks::value_expr(&vp, config, scalar_ref) else {
            continue;
        };
        let assign = if let Tref::Prim(Prim::Duration) = vp.target {
            helpers.duration_ms = true;
            let fail = checks::config_error(&format!(
                "format!(\"{path}: invalid duration {{:?}}\", {expr}.0)",
                path = vp.path,
            ));
            format!(
                "match parse_duration_ms(&{expr}.0) {{\n    Ok(ms) => {{\n        values.insert({path:?}.to_string(), serde_json::Value::from(ms));\n    }}\n    Err(_) => {{\n        {fail}\n    }}\n}}\n",
                path = vp.path,
            )
        } else {
            format!(
                "values.insert({path:?}.to_string(), {value});\n",
                path = vp.path,
                value = checks::value_cast(vp.target, &expr),
            )
        };
        match checks::presence_guard(entry, &vp, &expr, module, config) {
            Some(guard) => body.push_str(&format!("if {guard} {{\n{}}}\n", indent(&assign, 1))),
            None => body.push_str(&assign),
        }
    }

    body.push_str(&format!(
        "let runtime = tono_http_runtime::Runtime::new(tono_http_runtime::Options {{\n    base_url: String::new(),\n    client: s.client.clone(),\n    transport: s.transport.clone(),\n    headers: s.headers.clone(),\n    values,\n}})\n.map_err(|e| TonoError::Config(ConfigError {{ message: e.to_string() }}))?;\nOk({client} {{ settings: s, runtime: std::sync::Arc::new(runtime), hooks: None }})",
        client = n.client,
    ));

    body
}

/// Discard statements for the why-reasons nothing reads (mirrors Go's
/// `discard_unread_whys`): every declared source chain records why it came
/// up empty, but only a field something consumes gets a check reading that
/// reason back. Rather than predicting consumption ahead of the shared
/// plan's own choice, the reason is kept and explicitly discarded so a
/// why-var with no consumer is never merely an unused-but-harmless local
/// (idiomatic hygiene for the generated SDK, not a compile requirement of
/// this crate).
fn discard_unread_whys(body: &str, entry: &EntryModel<'_>) -> String {
    entry
        .fields
        .iter()
        .map(|f| why_var(&f.name))
        .filter(|why| is_written_never_read(body, why))
        .map(|why| format!("let _ = &{why};\n"))
        .collect()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether `ident` occurs in `body` and every occurrence is a write (a `let`
/// declaration or a plain reassignment `ident = ..`, not `==`).
fn is_written_never_read(body: &str, ident: &str) -> bool {
    let bytes = body.as_bytes();
    let mut found = false;
    let mut from = 0;
    while let Some(rel) = body[from..].find(ident) {
        let start = from + rel;
        let end = start + ident.len();
        from = end;
        let whole_word = (start == 0 || !is_ident_byte(bytes[start - 1]))
            && (end == bytes.len() || !is_ident_byte(bytes[end]));
        if !whole_word {
            continue;
        }
        found = true;
        let prefix = body[..start].trim_end();
        let is_decl = prefix.ends_with("let") || prefix.ends_with("mut");
        let rest = body[end..].trim_start_matches(' ');
        let is_reassign = rest.starts_with('=') && !rest.starts_with("==");
        if !(is_decl || is_reassign) {
            return false;
        }
    }
    found
}

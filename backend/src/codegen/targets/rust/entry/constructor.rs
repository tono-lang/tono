//! The generated constructor (a builder's `build` or a plain `new`): source
//! resolution in dependency order, the `client_init` bridge, the
//! consumed-chain requires, declared validation, the frozen runtime values,
//! and any bound `before_request`/`after_response` lifecycle hook wired into
//! the runtime's `Hooks` slot. Split out of `mod.rs` to stay under this
//! repo's per-file line ceiling; `construction_decls` is `mod.rs`'s only
//! caller.

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
///
/// With `test_seam`, the whole construction body moves into a `pub(crate)`
/// variant taking an `Option<Transport>`, and the public entry point
/// delegates with `None`: a generated test runs the real construction path
/// (resolution, client_init, validation) and only the transport is swapped,
/// after bespoke code ran, so the test sees exactly the request the SDK
/// would send. `pub(crate)` suffices because the generated test file is a
/// `#[cfg(test)]` module of the same crate.
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
    decls.extend(hook_wrapper_decls(bound));

    let body = resolution_body(
        entry, n, module, config, bound, helpers, multi, "self.", test_seam, &mut refs,
    );
    let plain_body = resolution_body(
        entry, n, module, config, bound, helpers, multi, "", test_seam, &mut refs,
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
        let doc = format!(
            "    /// Constructs {client}: the declared sources resolve top-down,\n    /// client_init runs on top (bespoke wins), then the declared validation.\n",
            client = n.client,
        );
        let constructors = if test_seam {
            let pass: Vec<String> = args
                .iter()
                .map(|f| arg_snake(&f.name, &f.traits, LANG))
                .collect();
            let comma = if params.is_empty() { "" } else { ", " };
            format!(
                "{doc}    pub fn new({params}) -> Result<Self, TonoError> {{\n        Self::new_with_transport(None{comma}{pass})\n    }}\n\n    /// `new` plus the transport seam the generated tests construct through:\n    /// a `Some` transport replaces whatever construction resolved, after\n    /// client_init ran, so a test answers canonically without a server.\n    pub(crate) fn new_with_transport(transport: Option<tono_http_runtime::Transport>{comma}{params}) -> Result<Self, TonoError> {{\n{body}\n    }}\n",
                params = params.join(", "),
                pass = pass.join(", "),
                body = indent(&plain_body, 2),
            )
        } else {
            format!(
                "{doc}    pub fn new({params}) -> Result<Self, TonoError> {{\n{body}\n    }}\n",
                params = params.join(", "),
                body = indent(&plain_body, 2),
            )
        };
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n{constructors}{methods}}}",
                client = n.client,
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

        let build_doc = "    /// Resolves the declared sources top-down, runs client_init on top\n    /// (bespoke wins), then the declared validation.\n";
        let build_fns = if test_seam {
            format!(
                "{build_doc}    pub fn build(self) -> Result<{client}, TonoError> {{\n        self.build_with_transport(None)\n    }}\n\n    /// `build` plus the transport seam the generated tests construct through:\n    /// a `Some` transport replaces whatever construction resolved, after\n    /// client_init ran, so a test answers canonically without a server.\n    pub(crate) fn build_with_transport(self, transport: Option<tono_http_runtime::Transport>) -> Result<{client}, TonoError> {{\n{body}\n    }}\n",
                client = n.client,
                body = indent(&body, 2),
            )
        } else {
            format!(
                "{build_doc}    pub fn build(self) -> Result<{client}, TonoError> {{\n{body}\n    }}\n",
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
/// With `test_seam`, a `transport` parameter override lands right before the
/// runtime freezes, so the test transport wins over anything bespoke set.
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
    test_seam: bool,
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
            refs,
        };
        let fields = plan::emit_fields(entry, module, &mut r, 1);
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
            refs,
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
        body.push_str(&format!(
            "let mut violations: Vec<Violation> = Vec::new();\n{guards}if !violations.is_empty() {{\n    return Err(TonoError::Validation(ValidationError {{ violations }}));\n}}\n",
        ));
    }

    body.push_str(&discard_unread_errs(&body, entry));

    body.push_str("let mut values = serde_json::Map::new();\n");
    for vp in entry.value_paths(module) {
        let scalar_ref = ref_is_enum(vp.target, module);
        let Some(expr) = checks::value_expr(&vp, config, scalar_ref) else {
            continue;
        };
        let assign = if let Tref::Prim(Prim::Duration) = vp.target {
            refs.push(shared_symbol("parse_duration_ms"));
            let fail = checks::config_error(&format!(
                "format!(\"{path}: invalid duration {{:?}}\", {expr}.0)",
                path = vp.path,
            ));
            format!(
                "match {parse}(&{expr}.0) {{\n    Ok(ms) => {{\n        values.insert({path:?}.to_string(), serde_json::Value::from(ms));\n    }}\n    Err(_) => {{\n        {fail}\n    }}\n}}\n",
                path = vp.path,
                parse = shared_slot("parse_duration_ms"),
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

    if test_seam {
        body.push_str(
            "if let Some(t) = transport {\n    s.transport = Some(t);\n    s.client = None;\n}\n",
        );
    }
    body.push_str(&format!(
        "let runtime = tono_http_runtime::Runtime::new(tono_http_runtime::Options {{\n    base_url: String::new(),\n    client: s.client.clone(),\n    transport: s.transport.clone(),\n    headers: s.headers.clone(),\n    values,\n}})\n.map_err(|e| TonoError::Config(ConfigError {{ message: e.to_string() }}))?;\nOk({client} {{ settings: s, runtime: std::sync::Arc::new(runtime), hooks: {hooks} }})",
        client = n.client,
        hooks = hooks_expr(bound),
    ));

    body
}

/// The name of the boundary-adapter free function for a bound lifecycle hook
/// slot (`__before_request_hook`, `__after_response_hook`).
fn hook_wrapper_name(slot: &str) -> String {
    format!("__{slot}_hook")
}

/// The wrapper fn decls for any bound `before_request`/`after_response`
/// hook: the runtime hands each hook slot an `Arc<dyn Fn(..) -> BoxFuture<..>>`,
/// so a bespoke `async fn` (the natural shape to write one in, since the
/// runtime always awaits it) is boxed into that shape here. No error
/// boundary is applied in the wrapper itself: a hook's error propagates raw
/// through `tono_http_runtime::ExecuteError` (already `Box<dyn Error + Send
/// + Sync>`, the same type a bespoke fn returns), and the classification
/// into a declared `TonoError` vs. a `ContractError` happens once,
/// centrally, at every op method's `Runtime::execute(...).map_err(...)`
/// boundary. Each wrapper is a standalone free function tied to no entry's
/// `Settings`, so unlike `client_init` it is wired regardless of `multi`.
fn hook_wrapper_decls(bound: &[BoundExtension<'_>]) -> Vec<Decl> {
    [
        ("before_request", "req", "CanonicalRequest"),
        ("after_response", "res", "CanonicalResponse"),
    ]
    .into_iter()
    .filter_map(|(slot, var, shape)| {
        let b = hook_binding(bound, slot)?;
        let wrapper = hook_wrapper_name(slot);
        Some(Decl::raw_with(
            format!(
                "fn {wrapper}({var}: tono_http_runtime::{shape}) -> tono_http_runtime::BoxFuture<'static, Result<tono_http_runtime::{shape}, tono_http_runtime::ExecuteError>> {{\n    Box::pin({sym}({var}))\n}}",
                sym = b.symbol,
            ),
            vec![Symbol::imported(b.symbol, use_path(b.module), b.symbol)],
        ))
    })
    .collect()
}

/// The `hooks:` field expression for the `Client` struct literal: `None`
/// when neither slot is bound (the common, current case), otherwise a
/// `Hooks` value with each bound slot wrapped and each unbound slot `None`.
fn hooks_expr(bound: &[BoundExtension<'_>]) -> String {
    let slot = |name: &str| -> String {
        if hook_binding(bound, name).is_some() {
            format!("Some(std::sync::Arc::new({}))", hook_wrapper_name(name))
        } else {
            "None".to_string()
        }
    };
    let before = slot("before_request");
    let after = slot("after_response");
    if before == "None" && after == "None" {
        "None".to_string()
    } else {
        format!(
            "Some(tono_http_runtime::Hooks {{ before_request: {before}, after_response: {after} }})"
        )
    }
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

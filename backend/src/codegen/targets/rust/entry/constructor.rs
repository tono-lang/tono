//! The generated constructor: source resolution in dependency order, the
//! consumed-chain requires, declared validation, and the frozen runtime
//! values.
//!
//! `new`/`build` is three steps. `new_settings` (a `pub(crate)` associated
//! function) resolves every construction value no foreign call reaches (the
//! `@arg` values, the env chains, the defaults, the derivations over those)
//! and runs the requires and the declared validation over them. `new`/
//! `build` then runs the foreign constructions in resolution order, each a
//! call to its own resolver (see `ext_resolver`), storing what an operation
//! reads and handing a forwarded handle straight to the resolver that
//! consumes it, and runs the requires and validation over those. `new_client`
//! freezes the runtime values and builds the client. A generated test
//! composes the same steps with its fakes in place of the resolver calls, so
//! no step carries a branch deciding between production and test.
//!
//! An entry with a declared test and a wire operation also keeps a
//! `pub(crate)` `_with_transport` variant of `new`/`build` (`new`/`build`
//! themselves delegate to it with `None`): the seam a hand-written,
//! same-crate test (the cross-runtime parity harness among them) constructs
//! through when it wants the real construction path but a fake transport.
//! A generated declared test never reaches this seam; see `vector_tests`.
//!
//! Split out of `mod.rs` to stay under this repo's per-file line ceiling;
//! `construction_decls` is `mod.rs`'s only caller.

use std::collections::BTreeMap;

use super::*;
use crate::codegen::entries::TailStep;

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

/// The transport override an `_with_transport` variant splices between the
/// settings/tail resolution and `new_client`: a `Some` transport replaces
/// whatever construction resolved onto `s` (and clears the native
/// `reqwest`-feature slot, so the two stay mutually exclusive the same way
/// `new_client` itself enforces). Empty when the entry has no transport
/// seam to carry.
fn transport_patch_lines(transport_seam: bool) -> &'static str {
    if transport_seam {
        "        if let Some(t) = transport {\n\
         \x20           s.transport = Some(t);\n\
         \x20           #[cfg(feature = \"reqwest\")]\n\
         \x20           {\n\
         \x20               s.client = None;\n\
         \x20           }\n\
         \x20       }\n"
    } else {
        ""
    }
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
#[allow(clippy::too_many_arguments)]
pub(super) fn construction_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
    has_tests: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    let mut refs = vec![];
    let mut resolve_fns: Vec<Decl> = Vec::new();
    let with_fields = entry.with_fields();
    let args = entry.args();
    let timeouts = timeout_fields(entry);
    let seam = has_retry(entry);
    let wire = crate::codegen::entries::has_wire_ops(module);
    // The transport seam a hand-written test constructs through (`new`/
    // `build` plus `_with_transport`): only worth emitting when there is a
    // declared test to build it for, and only meaningful when there is a
    // wire operation at all (nothing on `Settings` to override otherwise).
    let transport_seam = has_tests && wire;
    let split = entry.construction_split(module);
    let tail_fields: Vec<&EntryField> = split.tail.iter().map(TailStep::field).collect();
    // A call-sourced field's emitted call is always awaited (`resolve_call`),
    // so construction itself becomes async whenever the entry has one,
    // mirroring `impl_op`'s own `is_async` gate for op methods: Rust is an
    // async-lowering target, so a construction that depends on an external
    // call takes the language's idiomatic async form.
    let is_async = entry
        .declared()
        .iter()
        .any(|f| f.call.is_some() || f.handle_call.is_some());
    let effect = if is_async { "async " } else { "" };

    // The client struct itself: the resolved settings, the frozen transport
    // options (only when the entry declares a wire operation: an ext-only
    // entry has nothing to inject a transport into), one private field per
    // distinct `@timeout` path (converted to milliseconds eagerly; see
    // `new_client_decl`), and, when any operation retries, the crate-visible
    // timing seam the parity harness pins.
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
    let mut client_refs = vec![];
    let options_field_decl = if wire {
        client_refs.push(support_symbol("ClientOptions"));
        "    #[allow(dead_code)]\n    options: ClientOptions,\n"
    } else {
        ""
    };
    if seam {
        client_refs.push(shared_symbol("SleepFn"));
        client_refs.push(shared_symbol("RandomFn"));
    }
    decls.push(Decl::raw_with(
        format!(
            // An entry whose ops read no settings field directly (every wire
            // position a literal) never reads `settings` back after
            // construction; a bespoke `ext impl` op does. `dead_code` is
            // silenced per entry rather than omitting the field, so the
            // struct's shape stays uniform across entries regardless of
            // which ops happen to need it.
            "{doc}pub struct {client} {{\n    #[allow(dead_code)]\n    settings: {settings},\n{options_field_decl}{timeout_field_decls}{seam_field_decls}}}",
            client = n.client,
            settings = n.settings,
        ),
        client_refs,
    ));

    let settings_zero = zero_prelude(entry, n, module, config, wire, !split.settings.is_empty());
    decls.push(new_client_decl(
        entry, n, module, config, &timeouts, seam, wire,
    ));

    if with_fields.is_empty() {
        // No @with fields: a builder would have nothing to configure, so the
        // entry point is a plain associated function.
        let params: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "{}: {}",
                    arg_snake(&f.name, &f.traits, LANG),
                    ext::field_type(&f.target, module)
                )
            })
            .collect();
        let pass: Vec<String> = args
            .iter()
            .map(|f| arg_snake(&f.name, &f.traits, LANG))
            .collect();

        let mut settings_refs = refs.clone();
        let settings_body = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            "",
            &split.settings,
            &mut settings_refs,
            &mut resolve_fns,
        );
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n\
                 \x20   /// Resolves every construction value of {client} that no foreign\n\
                 \x20   /// call reaches, in declared order, then runs the consumed-chain\n\
                 \x20   /// requires and the declared validation over them.\n\
                 \x20   pub(crate) fn new_settings({params}) -> Result<{settings}, TonoError> {{\n\
                 {zero}\
                 {body}\
                 \x20       Ok(s)\n\
                 \x20   }}\n\
                 }}",
                client = n.client,
                settings = n.settings,
                params = params.join(", "),
                zero = indent(&settings_zero, 2),
                body = indent(&settings_body, 2),
            ),
            settings_refs,
        ));

        let mut tail_refs = refs.clone();
        let tail_body = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            "",
            &tail_fields,
            &mut tail_refs,
            &mut resolve_fns,
        );
        decls.append(&mut resolve_fns);
        let doc = format!(
            "    /// Constructs {client}: the declared sources resolve top-down, then\n    /// the declared validation.\n",
            client = n.client,
        );
        // An entry with no foreign construction never writes `s` again after
        // `new_settings` returns it: `mut` would be unused (and `-D warnings`
        // rejects it), so it is only spelled when the tail actually assigns,
        // or the transport seam below always does.
        let s_let = if tail_fields.is_empty() && !transport_seam {
            "let"
        } else {
            "let mut"
        };
        let body = format!(
            "        {s_let} s = Self::new_settings({pass})?;\n{tail}{transport_patch}\
             \x20       Self::new_client(s)\n",
            pass = pass.join(", "),
            tail = indent(&tail_body, 2),
            transport_patch = transport_patch_lines(transport_seam),
        );
        let methods = op_methods(entry, n, module, config, bound, &timeouts, &mut refs);
        let text = if transport_seam {
            refs.push(support_symbol("HttpTransport"));
            let comma = if params.is_empty() { "" } else { ", " };
            let awaited = if is_async { ".await" } else { "" };
            format!(
                "impl {client} {{\n{doc}    pub {effect}fn new({params}) -> Result<Self, TonoError> {{\n        Self::new_with_transport(None{comma}{pass}){awaited}\n    }}\n\n\
                 \x20   /// `new` plus the transport seam a hand-written test constructs\n\
                 \x20   /// through: a `Some` transport replaces whatever construction\n\
                 \x20   /// resolved, so the test answers canonically without a server.\n\
                 \x20   pub(crate) {effect}fn new_with_transport(transport: Option<HttpTransport>{comma}{params}) -> Result<Self, TonoError> {{\n{body}    }}\n{methods}}}",
                client = n.client,
                params = params.join(", "),
                pass = pass.join(", "),
            )
        } else {
            format!(
                "impl {client} {{\n{doc}    pub {effect}fn new({params}) -> Result<Self, TonoError> {{\n{body}    }}\n{methods}}}",
                client = n.client,
                params = params.join(", "),
            )
        };
        decls.push(Decl::raw_with(text, refs));
    } else {
        // A builder: the @arg fields are captured positionally at
        // `builder()`, the @with fields default to unset, and `build`
        // destructures the whole builder into locals before resolving
        // anything — `new_settings` then owns exactly the parameters it
        // reads (a foreign handle among them moves, never clones, the same
        // as `new`'s own bare arguments), and a `@with` field the tail step
        // still needs (backing a foreign construction, e.g. a call's own
        // fallback) survives as a plain local no borrow has to route around.
        let settings_with_fields: Vec<&EntryField> = with_fields
            .iter()
            .copied()
            .filter(|f| split.settings.iter().any(|sf| sf.name == f.name))
            .collect();

        let mut builder_fields: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "    {}: {},",
                    arg_snake(&f.name, &f.traits, LANG),
                    ext::field_type(&f.target, module)
                )
            })
            .collect();
        builder_fields.extend(with_fields.iter().map(|f| {
            format!(
                "    {}: Option<{}>,",
                arg_snake(&f.name, &f.traits, LANG),
                ext::field_type(&f.target, module)
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
                    ty = ext::field_type(&f.target, module),
                )
            })
            .collect();

        let settings_params: Vec<String> = args
            .iter()
            .map(|f| {
                format!(
                    "{}: {}",
                    arg_snake(&f.name, &f.traits, LANG),
                    ext::field_type(&f.target, module)
                )
            })
            .chain(settings_with_fields.iter().map(|f| {
                format!(
                    "{}: Option<{}>",
                    arg_snake(&f.name, &f.traits, LANG),
                    ext::field_type(&f.target, module)
                )
            }))
            .collect();
        let mut settings_refs = refs.clone();
        let settings_body = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            "",
            &split.settings,
            &mut settings_refs,
            &mut resolve_fns,
        );
        decls.push(Decl::raw_with(
            format!(
                "impl {client} {{\n\
                 \x20   /// Resolves every construction value of {client} that no foreign\n\
                 \x20   /// call reaches, in declared order, then runs the consumed-chain\n\
                 \x20   /// requires and the declared validation over them.\n\
                 \x20   pub(crate) fn new_settings({settings_params}) -> Result<{settings}, TonoError> {{\n\
                 {zero}\
                 {body}\
                 \x20       Ok(s)\n\
                 \x20   }}\n\
                 }}",
                client = n.client,
                settings = n.settings,
                settings_params = settings_params.join(", "),
                zero = indent(&settings_zero, 2),
                body = indent(&settings_body, 2),
            ),
            settings_refs,
        ));

        let tail_body = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            "",
            &tail_fields,
            &mut refs,
            &mut resolve_fns,
        );
        decls.append(&mut resolve_fns);

        let destructure: Vec<String> = args
            .iter()
            .chain(with_fields.iter())
            .map(|f| arg_snake(&f.name, &f.traits, LANG))
            .collect();
        let settings_pass: Vec<String> = args
            .iter()
            .chain(settings_with_fields.iter())
            .map(|f| arg_snake(&f.name, &f.traits, LANG))
            .collect();
        let build_doc = "    /// Resolves the declared sources top-down, then the declared\n    /// validation.\n";
        // An entry with no foreign construction never writes `s` again after
        // `new_settings` returns it: `mut` would be unused (and `-D warnings`
        // rejects it), so it is only spelled when the tail actually assigns,
        // or the transport seam below always does.
        let s_let = if tail_fields.is_empty() && !transport_seam {
            "let"
        } else {
            "let mut"
        };
        let build_body = format!(
            "        let {builder} {{ {destructure} }} = self;\n\
             \x20       {s_let} s = {client}::new_settings({settings_pass})?;\n{tail}{transport_patch}\
             \x20       {client}::new_client(s)\n",
            builder = n.builder,
            client = n.client,
            destructure = destructure.join(", "),
            settings_pass = settings_pass.join(", "),
            tail = indent(&tail_body, 2),
            transport_patch = transport_patch_lines(transport_seam),
        );
        let build_fns = if transport_seam {
            refs.push(support_symbol("HttpTransport"));
            let awaited = if is_async { ".await" } else { "" };
            format!(
                "{build_doc}    pub {effect}fn build(self) -> Result<{client}, TonoError> {{\n        self.build_with_transport(None){awaited}\n    }}\n\n\
                 \x20   /// `build` plus the transport seam a hand-written test constructs\n\
                 \x20   /// through: a `Some` transport replaces whatever construction\n\
                 \x20   /// resolved, so the test answers canonically without a server.\n\
                 \x20   pub(crate) {effect}fn build_with_transport(self, transport: Option<HttpTransport>) -> Result<{client}, TonoError> {{\n{build_body}    }}\n",
                client = n.client,
            )
        } else {
            format!(
                "{build_doc}    pub {effect}fn build(self) -> Result<{client}, TonoError> {{\n{build_body}    }}\n",
                client = n.client,
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
                    ext::field_type(&f.target, module)
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

/// The zero draft every construction value starts from: one entry per
/// *stored* field (a forwarded handle has no slot to zero: it never lives in
/// `Settings` at all) plus the transport slots, present only with a wire
/// operation. Shared by `new_settings` (both shapes): the settings step
/// zeroes every stored field, including the ones only the tail step
/// resolves, since they all live in the one struct `new_settings` returns.
fn zero_prelude(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    wire: bool,
    // Whether `new_settings` actually resolves any field afterward: with
    // none, `mut` would be unused (and `-D warnings` rejects it).
    settings_resolves_a_field: bool,
) -> String {
    let zeros: String = entry
        .stored(module)
        .iter()
        .map(|f| {
            format!(
                "{}: {}, ",
                field_snake_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
                zero_value(&f.target, module, config),
            )
        })
        .collect();
    let transport_zeros = if wire {
        "#[cfg(feature = \"reqwest\")]\n    client: None,\n    transport: None,\n    headers: std::collections::HashMap::new(),\n    "
    } else {
        ""
    };
    let s_let = if settings_resolves_a_field {
        "let mut"
    } else {
        "let"
    };
    format!(
        "{s_let} s = {settings} {{\n    {zeros}\n    {transport_zeros}}};\n",
        settings = n.settings,
        zeros = zeros.trim_end(),
    )
}

/// The resolution of `fields` (a half of the construction split), then the
/// consumed-chain requires and the declared validation of those fields,
/// relative to column zero. A foreign construction renders as a call to its
/// resolver through the plan's own leaf (`resolve::Resolver::call_assign`/
/// `handle_call_assign`, delegating to `ext_resolver::call_site`); everything
/// else through the shared plan. `resolve_fns` collects the standalone
/// `resolve_setting_*` functions a guaranteed env chain splits off, flushed
/// by the caller once both halves are built.
#[allow(clippy::too_many_arguments)]
pub(super) fn resolution_steps(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    multi: bool,
    arg_prefix: &'static str,
    fields: &[&EntryField],
    refs: &mut Vec<Symbol>,
    resolve_fns: &mut Vec<Decl>,
) -> String {
    let mut body = String::new();
    let keep: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();

    // A forwarded field also carrying `@with` resolves inside an `if`/`else`
    // (the injected value wins, the resolver call is the fallback): each arm
    // assigns the same local, so it has to be declared, unset, before the
    // branch rather than `let`-bound inside either arm (which would scope it
    // to that arm alone).
    for field in fields {
        if entry.is_forwarded(module, &field.name)
            && field.sources.iter().any(|s| matches!(s, Source::With))
        {
            body.push_str(&format!(
                "let {}: {};\n",
                ext_resolver::forwarded_local_ident(field),
                ext::field_type(&field.target, module),
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
            resolve_fns,
            multi,
            param_scope: false,
        };
        let rendered = plan::emit_fields_of(fields, entry, module, &mut r, 1);
        if !rendered.is_empty() {
            plan::push_gap(r.body);
            r.body.push_str(&rendered);
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
            param_scope: false,
        };
        let requires =
            plan::build_requires_for(entry, module, &mut r, &|head| keep.contains(&head));
        let text = plan::render(&requires, 0, &r);
        if !text.is_empty() {
            plan::push_gap(r.body);
            r.body.push_str(&text);
        }
    }

    let mut guards = String::new();
    for field in fields {
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

    body.push_str(&discard_unread_errs(&body, fields));
    body
}

/// `new_client`: the last step, over assembled settings. Each distinct
/// `@timeout` field converts once, eagerly, so a malformed value still fails
/// construction (`ConfigError`) rather than surfacing at the first call that
/// happens to need it; every other wire position reads the typed Settings
/// directly at the call site, so nothing else is frozen here. With a wire
/// operation the mutually exclusive transport slots are rejected here too, so
/// a misconfigured client fails to build instead of failing obscurely on its
/// first call.
#[allow(clippy::too_many_arguments)]
fn new_client_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    timeouts: &BTreeMap<String, String>,
    seam: bool,
    wire: bool,
) -> Decl {
    let mut refs = Vec::new();
    let mut body = String::new();

    for (path, field_name) in timeouts {
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

    let mut inits = String::from("settings: s");
    if wire {
        refs.push(support_symbol("ClientOptions"));
        refs.push(shared_symbol("check_transport"));
        body.push_str(
            "let options = ClientOptions {\n    #[cfg(feature = \"reqwest\")]\n    client: s.client.clone(),\n    transport: s.transport.clone(),\n    headers: s.headers.clone(),\n};\nif let Err(message) = check_transport(&options) {\n    return Err(TonoError::Config(ConfigError { message }));\n}\n",
        );
        inits.push_str(", options");
    }
    for field_name in timeouts.values() {
        inits.push_str(&format!(", {field_name}"));
    }
    if seam {
        refs.push(shared_symbol("default_sleep"));
        refs.push(shared_symbol("default_random"));
        inits.push_str(", sleep: default_sleep(), random: default_random()");
    }
    body.push_str(&format!("Ok({client} {{ {inits} }})", client = n.client));

    Decl::raw_with(
        format!(
            "impl {client} {{\n\
             \x20   /// Builds {client} over assembled settings: the runtime values\n\
             \x20   /// freeze here, after every construction value resolved.\n\
             \x20   pub(crate) fn new_client(s: {settings}) -> Result<Self, TonoError> {{\n{body}\n    }}\n\
             }}",
            client = n.client,
            settings = n.settings,
            body = indent(&body, 2),
        ),
        refs,
    )
}

/// Discard statements for the error vars nothing reads (mirrors Go's
/// `discard_unread_errs`): every declared source chain records its failure in
/// an error var, but only a field something consumes gets a check reading it
/// back. Rather than predicting consumption ahead of the shared plan's own
/// choice, the error is kept and explicitly discarded so an error var with no
/// consumer is never merely an unused-but-harmless local (idiomatic hygiene
/// for the generated SDK, not a compile requirement of this crate). Scoped to
/// `fields` (the half being resolved): a sibling half's error var lives in
/// the other function's own body, out of reach here.
fn discard_unread_errs(body: &str, fields: &[&EntryField]) -> String {
    fields
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

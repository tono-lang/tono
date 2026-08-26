//! The generated constructor: source resolution in dependency order, the
//! consumed-chain requires, declared validation, and the frozen runtime
//! values.
//!
//! `New` is three steps. `newSettings` resolves every construction value no
//! foreign call reaches (the `@arg` values, the env chains, the defaults,
//! the derivations over those) and runs the requires and the declared
//! validation over them. `New` then runs the foreign constructions in
//! resolution order, each a call to its own resolver (see `ext_resolver`),
//! storing what an operation reads and handing a forwarded handle straight
//! to the resolver that consumes it. `newClient` freezes the runtime values
//! and builds the client. A generated test composes the same three steps
//! with its fakes in place of the resolver calls, so no step carries a
//! branch deciding between production and test.

use super::resolve::config_errorf;
use super::*;
use crate::codegen::entries::has_wire_ops;
use crate::codegen::entries::plan;
use plan::push_gap;

/// The generated constructor and its two steps.
pub(super) fn new_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    multi: bool,
) -> Vec<Decl> {
    let mut decls = Vec::new();
    // The standalone `resolveSetting*` functions the plan splits off a
    // guaranteed env chain, emitted after the three steps.
    let mut split_off: Vec<Decl> = Vec::new();
    let split = entry.construction_split(module);

    // Constructor signature: positional @arg fields, then the options.
    let args = entry.args();
    let params: Vec<String> = args
        .iter()
        .map(|f| {
            // The storage-typed spelling ([`field_go_type_storage`]), not
            // the plain `go_type`: an `@arg` field's type reaches the
            // Settings struct verbatim, so its constructor parameter has to
            // name the exact same type the field is declared with (a
            // foreign opaque handle is tono's own generated interface
            // there, never the real package's local-looking type name).
            format!(
                "{} {}",
                plan::arg_camel(&f.name, &f.traits, LANG),
                field_go_type_storage(&f.target, module)
            )
        })
        .collect();
    let has_opts = !entry.with_fields().is_empty();
    let opts_param = if has_opts {
        format!(
            "{}opts ...{}",
            if params.is_empty() { "" } else { ", " },
            n.option
        )
    } else {
        String::new()
    };
    let pass_args: Vec<String> = args
        .iter()
        .map(|f| plan::arg_camel(&f.name, &f.traits, LANG))
        .collect();
    let pass_call = {
        let mut parts = pass_args.clone();
        if has_opts {
            parts.push("opts...".to_string());
        }
        parts.join(", ")
    };
    let settings_fn = settings_fn_name(n);
    let client_fn = client_fn_name(n);

    // The `@with` carrier is folded from the options wherever a `@with`
    // field resolves: in the settings step, and again in `New` when a
    // foreign construction has an injected shortcut.
    let fold_opts = format!(
        "\tw := {carrier}{{}}\n\tfor _, opt := range opts {{\n\t\topt(&w)\n\t}}\n",
        carrier = n.carrier
    );
    let tail_has_with = split.tail.iter().any(|s| {
        s.field()
            .sources
            .iter()
            .any(|src| matches!(src, Source::With))
    });

    // newSettings: the shared step.
    {
        let mut refs = Vec::new();
        for field in entry.declared() {
            push_field_type_symbols(&field.target, module, &mut refs);
        }
        let mut body = String::new();
        if has_opts {
            body.push_str(&fold_opts);
        }
        body.push_str(&format!(
            "\ts := {settings}{{{init}}}\n",
            settings = n.settings,
            init = if has_wire_ops(module) {
                "Headers: map[string]string{}"
            } else {
                ""
            },
        ));
        let keep: Vec<&str> = split.settings.iter().map(|f| f.name.as_str()).collect();
        let (steps, resolve_fns) = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            &mut refs,
            &split.settings,
            &keep,
            "s",
        );
        body.push_str(&steps);
        body.push_str("\treturn s, nil\n");
        split_off.extend(resolve_fns);
        decls.push(Decl::raw_with(
            format!(
                "// {settings_fn} resolves every construction value of {client} that no\n\
                 // foreign call reaches, in declared order, then runs the consumed-chain\n\
                 // requires and the declared validation over them.\n\
                 func {settings_fn}({params}{opts_param}) ({settings}, error) {{\n{body}}}",
                client = n.client,
                params = params.join(", "),
                settings = n.settings,
            ),
            refs,
        ));
    }

    // New: the settings step, the foreign constructions, the client.
    {
        let mut refs = Vec::new();
        let mut body = format!(
            "\ts, err := {settings_fn}({pass_call})\n\tif err != nil {{\n\t\treturn nil, err\n\t}}\n"
        );
        if tail_has_with {
            body.push_str(&fold_opts);
        }
        let tail_fields: Vec<&EntryField> = split.tail.iter().map(|s| s.field()).collect();
        let keep: Vec<&str> = tail_fields.iter().map(|f| f.name.as_str()).collect();
        let (steps, resolve_fns) = resolution_steps(
            entry,
            module,
            config,
            helpers,
            multi,
            &mut refs,
            &tail_fields,
            &keep,
            "nil",
        );
        body.push_str(&steps);
        body.push_str(&format!("\treturn {client_fn}(s)\n"));
        split_off.extend(resolve_fns);
        let doc = format!(
            "// {new_fn} constructs {client}: positional @arg values, options for @with,\n\
             // declared sources resolved top-down, then the declared validation.\n",
            new_fn = n.new_fn,
            client = n.client,
        );
        decls.insert(
            0,
            Decl::raw_with(
                format!(
                    "{doc}func {new_fn}({params}{opts_param}) (*{client}, error) {{\n{body}}}",
                    new_fn = n.new_fn,
                    client = n.client,
                    params = params.join(", "),
                ),
                refs,
            ),
        );
    }

    decls.push(client_decl(entry, n, module, config));
    decls.extend(split_off);
    decls
}

/// The resolution of `fields` (a half of the construction split), then the
/// consumed-chain requires and the declared validation of those fields, as
/// the body lines of the function resolving them, plus the standalone
/// resolver functions the plan split off (a guaranteed env chain's own
/// `resolveSetting*`). A foreign construction renders as a call to its
/// resolver through the plan's own leaf; everything else through the plan.
/// `fail_value` is what a failure returns beside its error (the settings
/// step returns the settings it was building, `New` returns nil).
#[allow(clippy::too_many_arguments)]
fn resolution_steps(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    helpers: &mut Helpers,
    multi: bool,
    refs: &mut Vec<Symbol>,
    fields: &[&EntryField],
    keep: &[&str],
    fail_value: &'static str,
) -> (String, Vec<Decl>) {
    let en = error_names();
    let mut body = String::new();
    let mut resolve_fns: Vec<Decl> = Vec::new();
    let mut r = Resolver {
        entry,
        module,
        config,
        helpers,
        refs,
        body: &mut body,
        resolve_fns: &mut resolve_fns,
        multi,
        fail_value,
    };
    let rendered = plan::emit_fields_of(fields, entry, module, &mut r, 1);
    if !rendered.is_empty() {
        push_gap(r.body);
        r.body.push_str(&rendered);
    }

    // Consumed chains must hold a value once construction finishes; an absent
    // one reports the chain at this single point instead of failing the first
    // call obscurely. The selection of which fields need a check lives in the
    // shared plan; this target only spells each check (and pulls the errors
    // import it needs). A check reads the head's error variable, so it sits in
    // the function that resolved the head.
    let requires = plan::build_requires_for(entry, module, &mut r, &|head| keep.contains(&head));
    let text = plan::render(&requires, 1, &r);
    if !text.is_empty() {
        push_gap(r.body);
        r.body.push_str(&text);
    }

    // Declared validation runs last, over what resolved.
    let mut guards = String::new();
    for field in fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = crate::codegen::entries::field_as_member(field);
        for line in validation::guard_lines(&[member], &GoVal, "s.", config, LANG) {
            // Presence is judged off the resolved value itself, never the
            // declared chain's error var. A numeric zero can be a
            // legitimately resolved value, so its guard only skips when the
            // chain reported absent AND left the zero in place.
            let guard = match plan::presence_kind(field, entry, module) {
                plan::Presence::Always => line.condition.clone(),
                plan::Presence::String => format!(
                    "s.{} != {} && {}",
                    field_pascal_ren(
                        &field.name,
                        rename_of(&field.traits, LANG).as_deref(),
                        config
                    ),
                    cast_string(&field.target, "\"\""),
                    line.condition
                ),
                plan::Presence::Bytes => format!(
                    "len(s.{}) != 0 && {}",
                    field_pascal_ren(
                        &field.name,
                        rename_of(&field.traits, LANG).as_deref(),
                        config
                    ),
                    line.condition
                ),
                plan::Presence::Numeric => format!(
                    "({err} == nil || s.{ident} != 0) && {}",
                    line.condition,
                    err = err_var(&field.name),
                    ident = field_pascal_ren(
                        &field.name,
                        rename_of(&field.traits, LANG).as_deref(),
                        config
                    ),
                ),
            };
            guards.push_str(&format!(
                "\tif {guard} {{\n\t\tviolations = append(violations, {violation}{{Field: {field:?}, Constraint: {constraint:?}, Message: {message:?}}})\n\t}}\n",
                violation = en.violation,
                field = line.field,
                constraint = line.constraint,
                message = line.message,
            ));
        }
    }
    if !guards.is_empty() {
        push_gap(r.body);
        r.body.push_str(&format!(
            "\tviolations := []{violation}{{}}\n{guards}\tif len(violations) > 0 {{\n\t\treturn {fail_value}, &{validation}{{Violations: violations}}\n\t}}\n",
            violation = en.violation,
            validation = en.validation,
        ));
    }

    // Every error var has been written and every check that reads one has been
    // emitted, so this is the point where an unread one can be told apart.
    let discard = discard_unread_errs(r.body, fields);
    r.body.push_str(&discard);
    (body, resolve_fns)
}

/// `newClient`: the last step, over assembled settings. Each distinct
/// `@timeout` field converts once, eagerly, so a malformed value still fails
/// construction (ConfigError) rather than surfacing at the first call that
/// happens to need it; every other wire position reads the typed Settings
/// directly at the call site, so nothing else is frozen here. With a wire
/// operation the mutually exclusive transport slots are rejected here too,
/// so a misconfigured client fails to build instead of failing obscurely on
/// its first call.
fn client_decl(entry: &EntryModel<'_>, n: &Names, module: &Module, config: &CasingConfig) -> Decl {
    let mut refs = Vec::new();
    let mut body = String::new();
    let mut client_fields = vec!["settings: s".to_string()];
    for (key, segments) in surface::timeout_paths(entry) {
        let Some(vp) = entry
            .value_paths(module)
            .into_iter()
            .find(|vp| vp.path == key)
        else {
            continue;
        };
        let scalar_ref = ref_is_enum(vp.target, module);
        let Some(expr) = value_expr(&vp, config, scalar_ref) else {
            continue;
        };
        let ident = super::timeout_field_ident(entry, config, &segments);
        refs.push(import("time", "time"));
        refs.push(import("fmt", "fmt"));
        let fail = config_errorf(
            "nil",
            &format!(
                "\"{path}: invalid duration %q\", string({expr})",
                path = vp.path
            ),
        );
        let parse = format!(
            "\t\td, err := time.ParseDuration(string({expr}))\n\
             \t\tif err != nil {{\n\t\t\t{fail}\n\t\t}}\n\
             \t\t{ident} = d\n",
        );
        body.push_str(&format!("\t{ident} := time.Duration(0)\n"));
        match presence_guard(entry, &vp, &expr) {
            Some(guard) => body.push_str(&format!("\tif {guard} {{\n{parse}\t}}\n")),
            None => body.push_str(&format!("\t{{\n{parse}\t}}\n")),
        }
        client_fields.push(format!("{ident}: {ident}"));
    }
    if has_wire_ops(module) {
        refs.push(import("errors", "errors"));
        body.push_str(
            "\tif s.HTTPClient != nil && s.Transport != nil {\n\
             \t\treturn nil, errors.New(\"Settings.HTTPClient and Settings.Transport are mutually exclusive: set the native slot or the canonical slot, not both\")\n\
             \t}\n",
        );
    }
    body.push_str(&format!(
        "\treturn &{client}{{{fields}}}, nil\n",
        client = n.client,
        fields = client_fields.join(", "),
    ));
    let client_fn = client_fn_name(n);
    Decl::raw_with(
        format!(
            "// {client_fn} builds {client} over assembled settings: the runtime values\n\
             // freeze here, after every construction value resolved.\n\
             func {client_fn}(s {settings}) (*{client}, error) {{\n{body}}}",
            client = n.client,
            settings = n.settings,
        ),
        refs,
    )
}

/// The unexported name of the shared settings step: `newSettings`, or
/// `newAdminSettings` beside `NewAdmin` in a multi-entry module.
pub(super) fn settings_fn_name(n: &Names) -> String {
    format!("new{}", n.settings)
}

/// The unexported name of the last step: `newClient` beside `New`, or
/// `newAdmin` beside `NewAdmin`.
pub(super) fn client_fn_name(n: &Names) -> String {
    format!("new{}", n.client)
}

pub(super) use plan::err_var;

/// Discard statements for the error vars nothing reads.
///
/// Every declared source chain records its failure in an error var, but only a
/// field something consumes gets a check that reads it back. A field with a
/// chain and no consumer therefore leaves a variable that is assigned and never
/// read, which Go rejects outright. Rather than making the resolver predict
/// consumption (the checks are chosen by the shared plan, further down), the
/// error is kept and explicitly discarded: the chain still records why it
/// failed, and the emitted source compiles.
fn discard_unread_errs(body: &str, fields: &[&EntryField]) -> String {
    fields
        .iter()
        .map(|f| err_var(&f.name))
        .filter(|err| {
            plan::is_written_never_read(body, err, |prefix| prefix.ends_with("var"), Some(":="))
        })
        .map(|err| format!("\t_ = {err}\n"))
        .collect()
}

/// The Go expression reading a resolved value path off the Settings, or `None`
/// for a path that has no scalar runtime value (a whole config/struct field).
/// A named reference freezes when it resolves to a scalar (an enum is a
/// branded string), which is what `scalar_ref` says.
pub(super) fn value_expr(
    vp: &crate::codegen::entries::ValuePath<'_>,
    config: &CasingConfig,
    scalar_ref: bool,
) -> Option<String> {
    match (&vp.member, vp.target) {
        (None, Tref::Ref { .. }) if !scalar_ref => None,
        (None, Tref::Map(_, _)) | (None, Tref::List(_)) => None,
        (None, _) => Some(format!(
            "s.{}",
            field_pascal_ren(
                &vp.field.name,
                rename_of(&vp.field.traits, LANG).as_deref(),
                config
            )
        )),
        (Some(member), t) => {
            if matches!(t, Tref::Ref { .. }) && !scalar_ref {
                return None;
            }
            if matches!(t, Tref::Map(_, _) | Tref::List(_)) {
                return None;
            }
            Some(format!(
                "s.{}.{}",
                field_pascal_ren(
                    &vp.field.name,
                    rename_of(&vp.field.traits, LANG).as_deref(),
                    config
                ),
                field_pascal(member, config)
            ))
        }
    }
}

/// The presence condition guarding a value entry, or `None` when the value is
/// always frozen. The decision (a guaranteed non-member field is always present;
/// a non-string value freezes unconditionally) is shared; this target spells
/// only the comparison.
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
) -> Option<String> {
    crate::codegen::entries::needs_presence_guard(entry, vp)
        .then(|| format!("{expr} != {}", cast_string(vp.target, "\"\"")))
}

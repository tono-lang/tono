//! The generated constructor: source resolution in dependency order, the
//! client_init bridge, the consumed-chain requires, declared validation,
//! and the frozen runtime values.

use std::collections::HashMap;

use super::resolve::{config_errorf, FieldOverride};
use super::*;
use crate::codegen::entries::plan;
use plan::push_gap;

/// The seam constructor's override parameter name for a field with its own
/// `extern` construction call.
pub(super) fn override_param_name(field_name: &str) -> String {
    format!("{}Override", camel(field_name))
}

/// The entry fields a declared test may stub during construction: every
/// field with its own `extern` construction call, in declared order.
pub(super) fn overridable_fields<'a>(entry: &'a EntryModel<'a>) -> Vec<&'a EntryField> {
    entry
        .fields
        .iter()
        .copied()
        .filter(|f| f.call.is_some())
        .collect()
}

/// The generated constructor. The body follows the declared order exactly:
/// sources resolve top-down, `client_init` runs over the result (bespoke
/// wins), the consumed chains and declared constraints validate last, and the
/// resolved values are frozen into the runtime options.
///
/// With `test_seam`, the whole body moves into an unexported variant taking a
/// transport, and the public constructor delegates with none: a generated test
/// runs the real construction path (resolution, client_init, validation) and
/// only the transport is swapped, after bespoke code ran, so the test sees
/// exactly the request the SDK would send.
#[allow(clippy::too_many_arguments)]
pub(super) fn new_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
    test_seam: bool,
) -> Vec<Decl> {
    let en = error_names();
    let mut refs = Vec::new();
    let mut resolve_fns: Vec<Decl> = Vec::new();
    // Every declared field's type can surface in the body as a zero value, a
    // default cast or a parse, all of them opaque text, so the references are
    // declared once here rather than at each spelling.
    for field in entry.declared() {
        push_field_type_symbols(&field.target, module, &mut refs);
    }
    let mut body = String::new();

    // Constructor signature: positional @arg fields, then the options.
    let args = entry.args();
    let params: Vec<String> = args
        .iter()
        .map(|f| {
            format!(
                "{} {}",
                plan::arg_camel(&f.name, &f.traits, LANG),
                go_type(&f.target)
            )
        })
        .collect();
    let opts_param = if entry.with_fields().is_empty() {
        String::new()
    } else {
        format!(
            "{}opts ...{}",
            if params.is_empty() { "" } else { ", " },
            n.option
        )
    };

    if !entry.with_fields().is_empty() {
        body.push_str(&format!(
            "\tw := {carrier}{{}}\n\tfor _, opt := range opts {{\n\t\topt(&w)\n\t}}\n",
            carrier = n.carrier
        ));
    }
    body.push_str(&format!(
        "\ts := {settings}{{Headers: map[string]string{{}}}}\n",
        settings = n.settings
    ));

    // The declared-test seam gets one optional override per field with its
    // own `extern` construction call: a non-nil value skips the real call,
    // which is what lets a hermetic test avoid the real library. A
    // foreign-handle field's override carries its own interface type
    // directly (already nilable); every other field's override is a
    // pointer, since the field's own zero value cannot double as "unset".
    let overrides: HashMap<String, FieldOverride> = if test_seam {
        overridable_fields(entry)
            .into_iter()
            .map(|f| {
                let pointer = ext::foreign_handle(&f.target, module).is_none();
                (
                    f.name.clone(),
                    FieldOverride {
                        var: override_param_name(&f.name),
                        pointer,
                    },
                )
            })
            .collect()
    } else {
        HashMap::new()
    };

    let mut r = Resolver {
        entry,
        module,
        config,
        helpers,
        refs: &mut refs,
        body: &mut body,
        resolve_fns: &mut resolve_fns,
        multi,
        overrides: &overrides,
    };
    let fields = plan::emit_fields(entry, module, &mut r, 1);
    if !fields.is_empty() {
        push_gap(r.body);
        r.body.push_str(&fields);
    }

    // client_init runs over the resolved Settings; bespoke wins.
    if hook_binding(bound, "client_init").is_some() && !multi {
        push_gap(&mut body);
        body.push_str(&format!(
            "\tif err := {}(&s); err != nil {{\n\t\treturn nil, err\n\t}}\n",
            hook_wrapper_name("client_init")
        ));
    }

    // Consumed chains must hold a value once construction finishes; an absent
    // one reports the chain at this single point instead of failing the first
    // call obscurely. Every check reads the resolved value (client_init ran
    // already, bespoke wins), so the error var only decorates the error. The
    // selection of which fields need a check lives in the shared plan; this
    // target only spells each check (and pulls the errors import it needs).
    {
        let mut r = Resolver {
            entry,
            module,
            config,
            helpers,
            refs: &mut refs,
            body: &mut body,
            resolve_fns: &mut resolve_fns,
            multi,
            overrides: &overrides,
        };
        let requires = plan::build_requires(entry, module, &mut r);
        let text = plan::render(&requires, 1, &r);
        if !text.is_empty() {
            push_gap(r.body);
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
        for line in validation::guard_lines(&[member], &GoVal, "s.", config, LANG) {
            // The check reads the value bespoke left in place (client_init ran
            // already, bespoke wins), so presence is judged off the value
            // itself, never the declared chain's error var. A numeric zero can
            // be a legitimately resolved value, so its guard only skips when the
            // chain reported absent AND the bridge left the zero in place.
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
        push_gap(&mut body);
        body.push_str(&format!(
            "\tviolations := []{violation}{{}}\n{guards}\tif len(violations) > 0 {{\n\t\treturn nil, &{validation}{{Violations: violations}}\n\t}}\n",
            violation = en.violation,
            validation = en.validation,
        ));
    }

    // Every error var has been written and every check that reads one has been
    // emitted, so this is the point where an unread one can be told apart.
    body.push_str(&discard_unread_errs(&body, entry));

    let mut client_fields = vec!["settings: s".to_string()];

    // Each distinct @timeout field converts once, eagerly, so a malformed
    // value still fails construction (ConfigError) rather than surfacing at
    // the first call that happens to need it. Every other wire position
    // (endpoint, @header, path templates, @retry) reads the typed Settings
    // directly at the call site, so nothing else is frozen here.
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
        let fail = config_errorf(&format!(
            "\"{path}: invalid duration %q\", string({expr})",
            path = vp.path,
        ));
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

    if hook_binding(bound, "before_request").is_some()
        || hook_binding(bound, "after_response").is_some()
    {
        let mut slots = Vec::new();
        if hook_binding(bound, "before_request").is_some() {
            slots.push(format!(
                "BeforeRequest: {}",
                hook_wrapper_name("before_request")
            ));
        }
        if hook_binding(bound, "after_response").is_some() {
            slots.push(format!(
                "AfterResponse: {}",
                hook_wrapper_name("after_response")
            ));
        }
        refs.push(super::shared_symbol("Hooks"));
        client_fields.push(format!(
            "hooks: &{hooks}{{{slots}}}",
            hooks = super::shared_slot("Hooks"),
            slots = slots.join(", "),
        ));
    }

    if test_seam {
        body.push_str(
            "\tif canonical != nil {\n\t\ts.Transport = canonical\n\t\ts.HTTPClient = nil\n\t}\n",
        );
    }
    // The mutually exclusive transport slots are rejected at construction, so
    // a misconfigured client fails to build instead of failing obscurely on
    // its first call.
    refs.push(import("errors", "errors"));
    body.push_str(
        "\tif s.HTTPClient != nil && s.Transport != nil {\n\
         \t\treturn nil, errors.New(\"Settings.HTTPClient and Settings.Transport are mutually exclusive: set the native slot or the canonical slot, not both\")\n\
         \t}\n",
    );
    body.push_str(&format!(
        "\treturn &{client}{{{fields}}}, nil\n",
        client = n.client,
        fields = client_fields.join(", "),
    ));

    let doc = format!(
        "// {new_fn} constructs {client}: positional @arg values, options for @with,\n\
         // declared sources resolved top-down, client_init on top (bespoke wins),\n\
         // then the declared validation.\n",
        new_fn = n.new_fn,
        client = n.client,
    );
    let text = if test_seam {
        let seam_fn = seam_fn_name(&n.new_fn);
        let pass_args: Vec<String> = args
            .iter()
            .map(|f| plan::arg_camel(&f.name, &f.traits, LANG))
            .collect();
        let overridable = overridable_fields(entry);
        // Go requires the variadic `opts ...Option` parameter last, so every
        // override is threaded between the positional args and it, in both
        // the signature and the wrapper's forwarding call.
        let mut seam_param_parts: Vec<String> = params.clone();
        let mut override_nil_parts: Vec<&str> = Vec::new();
        for f in &overridable {
            push_field_type_symbols(&f.target, module, &mut refs);
            let storage = field_go_type_storage(&f.target, module);
            let ty = if ext::foreign_handle(&f.target, module).is_none() {
                format!("*{storage}")
            } else {
                storage
            };
            seam_param_parts.push(format!("{} {ty}", override_param_name(&f.name)));
            override_nil_parts.push("nil");
        }
        if !entry.with_fields().is_empty() {
            seam_param_parts.push(format!("opts ...{}", n.option));
        }
        let seam_params = if seam_param_parts.is_empty() {
            String::new()
        } else {
            format!(", {}", seam_param_parts.join(", "))
        };
        let mut pass_parts = pass_args.clone();
        pass_parts.extend(override_nil_parts.iter().map(|s| s.to_string()));
        if !entry.with_fields().is_empty() {
            pass_parts.push("opts...".to_string());
        }
        let pass_call = if pass_parts.is_empty() {
            String::new()
        } else {
            format!(", {}", pass_parts.join(", "))
        };
        refs.push(super::support_symbol("HTTPTransport"));
        format!(
            "{doc}func {new_fn}({params}{opts_param}) (*{client}, error) {{\n\
             \treturn {seam_fn}(nil{pass_call})\n\
             }}\n\n\
             // {seam_fn} is {new_fn} plus the seams the generated tests use: a non-nil\n\
             // canonical transport replaces whatever construction resolved, after\n\
             // client_init ran; a non-nil field override skips that field's own real\n\
             // `extern` construction call outright, so a hermetic test never reaches\n\
             // the real library, not just avoids calling it at runtime.\n\
             func {seam_fn}(canonical {transport}{seam_params}) (*{client}, error) {{\n{body}}}",
            new_fn = n.new_fn,
            client = n.client,
            params = params.join(", "),
            transport = super::shared_slot("HTTPTransport"),
        )
    } else {
        format!(
            "{doc}func {new_fn}({params}{opts_param}) (*{client}, error) {{\n{body}}}",
            new_fn = n.new_fn,
            client = n.client,
            params = params.join(", "),
        )
    };
    resolve_fns.push(Decl::raw_with(text, refs));
    resolve_fns
}

/// The unexported name of the constructor variant carrying the transport seam:
/// `New` -> `newWithTransport`, `NewAdmin` -> `newAdminWithTransport`.
pub(super) fn seam_fn_name(new_fn: &str) -> String {
    let mut chars = new_fn.chars();
    let lowered = match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => String::new(),
    };
    format!("{lowered}WithTransport")
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
fn discard_unread_errs(body: &str, entry: &EntryModel<'_>) -> String {
    entry
        .fields
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

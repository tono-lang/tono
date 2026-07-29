//! The generated constructor: source resolution in dependency order, the
//! client_init bridge, the consumed-chain requires, declared validation,
//! and the frozen runtime values.

use super::resolve::config_errorf;
use super::*;
use crate::codegen::entries::plan;

/// The generated constructor. The body follows the declared order exactly:
/// sources resolve top-down, `client_init` runs over the result (bespoke
/// wins), the consumed chains and declared constraints validate last, and the
/// resolved values are frozen into the runtime options.
#[allow(clippy::too_many_arguments)]
pub(super) fn new_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    helpers: &mut Helpers,
    multi: bool,
) -> Decl {
    let en = error_names();
    let mut refs = vec![runtime_symbol()];
    // Every declared field's type can surface in the body as a zero value, a
    // default cast or a parse, all of them opaque text, so the references are
    // declared once here rather than at each spelling.
    for field in entry.declared() {
        push_type_symbols(&field.target, &mut refs);
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

    let mut r = Resolver {
        entry,
        module,
        config,
        helpers,
        refs: &mut refs,
        body: &mut body,
    };
    let fields = plan::emit_fields(entry, module, &mut r, 1);
    r.body.push_str(&fields);

    // client_init runs over the resolved Settings; bespoke wins.
    if hook_binding(bound, "client_init").is_some() && !multi {
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
        };
        let requires = plan::build_requires(entry, module, &mut r);
        let text = plan::render(&requires, 1, &r);
        r.body.push_str(&text);
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
        body.push_str(&format!(
            "\tviolations := []{violation}{{}}\n{guards}\tif len(violations) > 0 {{\n\t\treturn nil, &{validation}{{Violations: violations}}\n\t}}\n",
            violation = en.violation,
            validation = en.validation,
        ));
    }

    // Every error var has been written and every check that reads one has been
    // emitted, so this is the point where an unread one can be told apart.
    body.push_str(&discard_unread_errs(&body, entry));

    // Freeze the resolved values for the runtime's ref positions.
    body.push_str("\tvalues := map[string]any{}\n");
    for vp in entry.value_paths(module) {
        // An enum-typed leaf is a branded string wherever it sits (a field or
        // a composed/structured member): it freezes like any other scalar the
        // descriptor's refs can name.
        let scalar_ref = ref_is_enum(vp.target, module);
        let Some(expr) = value_expr(&vp, config, scalar_ref) else {
            continue;
        };

        let assign = if let Tref::Prim(Prim::Duration) = vp.target {
            helpers.duration_ms = true;
            refs.push(import("fmt", "fmt"));
            refs.push(super::shared_symbol("DurationMs"));
            let fail = config_errorf(&format!(
                "\"{path}: invalid duration %q\", string({expr})",
                path = vp.path,
            ));
            format!(
                "\t{{\n\t\tms, err := {duration}(string({expr}))\n\t\tif err != nil {{\n\t\t\t{fail}\n\t\t}}\n\t\tvalues[{path:?}] = ms\n\t}}\n",
                duration = super::shared_slot("DurationMs"),
                path = vp.path,
            )
        } else {
            format!(
                "\tvalues[{path:?}] = {value}\n",
                path = vp.path,
                value = value_cast(vp.target, &expr)
            )
        };
        match presence_guard(entry, &vp, &expr) {
            Some(guard) => body.push_str(&format!(
                "\tif {guard} {{\n{assign}\t}}\n",
                assign = indent(&assign),
            )),
            None => body.push_str(&assign),
        }
    }

    // The runtime: one per client; the per-operation endpoint resolves from
    // the descriptor's ref against these values.
    body.push_str(
        "\truntime, err := tonohttp.New(tonohttp.Options{Client: s.HTTPClient, Transport: s.Transport, Headers: s.Headers, Values: values})\n\
         \tif err != nil {\n\t\treturn nil, err\n\t}\n",
    );
    let hooks = {
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
        if slots.is_empty() {
            "nil".to_string()
        } else {
            format!("&tonohttp.Hooks{{{}}}", slots.join(", "))
        }
    };
    body.push_str(&format!(
        "\treturn &{client}{{settings: s, runtime: runtime, hooks: {hooks}}}, nil\n",
        client = n.client,
    ));

    let text = format!(
        "// {new_fn} constructs {client}: positional @arg values, options for @with,\n\
         // declared sources resolved top-down, client_init on top (bespoke wins),\n\
         // then the declared validation.\n\
         func {new_fn}({params}{opts_param}) (*{client}, error) {{\n{body}}}",
        new_fn = n.new_fn,
        client = n.client,
        params = params.join(", "),
    );
    Decl::raw_with(text, refs)
}

pub(super) fn indent(block: &str) -> String {
    block
        .lines()
        .map(|l| format!("\t{l}\n"))
        .collect::<String>()
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

/// The cast that makes a resolved value directly usable by the runtime's
/// value positions: integers widen to int64, branded strings flatten.
pub(super) fn value_cast(t: &Tref, expr: &str) -> String {
    match t {
        Tref::Prim(
            Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64,
        ) => format!("int64({expr})"),
        Tref::Prim(Prim::Float) => format!("float64({expr})"),
        Tref::Prim(Prim::Bool | Prim::String | Prim::Uuid) => expr.to_string(),
        Tref::Prim(Prim::Timestamp | Prim::Date) | Tref::Ref { .. } => {
            format!("string({expr})")
        }
        _ => expr.to_string(),
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

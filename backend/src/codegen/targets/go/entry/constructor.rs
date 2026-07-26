//! The generated constructor: source resolution in dependency order, the
//! client_init bridge, the consumed-chain requires, declared validation,
//! and the frozen runtime values.

use super::*;

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
    let mut body = String::new();

    // Constructor signature: positional @arg fields, then the options.
    let args = entry.args();
    let params: Vec<String> = args
        .iter()
        .map(|f| format!("{} {}", camel(&f.name), go_type(&f.target)))
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
    for field in &entry.fields {
        r.emit_field(field);
    }

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
    // already, bespoke wins), so the why-reason only decorates the error.
    let mut needs_errors = false;
    for path in entry.consumed_field_paths() {
        let Some(head) = path.first() else {
            continue;
        };
        let Some(field) = entry.fields.iter().find(|f| f.name == *head) else {
            continue;
        };
        let shape = entry.field_shape(field, module);
        if path.len() > 1 && matches!(shape, FieldShape::Config(_) | FieldShape::Structured(_)) {
            // A consumed member of a composed or decoded field: the leaf value
            // itself must be there (there is no member-level why to report).
            let leaf = entry.path_type(&path, module);
            if !string_like(&leaf) {
                continue;
            }
            needs_errors = true;
            body.push_str(&format!(
                "\tif s.{head_ident}.{member_ident} == {zero} {{\n\
                 \t\treturn nil, errors.New(\"{name}: no value\")\n\
                 \t}}\n",
                head_ident = field_pascal(head, config),
                member_ident = field_pascal(&path[1], config),
                zero = cast_string(&leaf, "\"\""),
                name = path.join("."),
            ));
            continue;
        }
        if !matches!(shape, FieldShape::Scalar) || entry.is_guaranteed(field) {
            continue;
        }
        if string_like(&field.target) {
            needs_errors = true;
            body.push_str(&format!(
                "\tif s.{ident} == {zero} {{\n\
                 \t\twhy := {why}\n\
                 \t\tif why == \"\" {{\n\t\t\twhy = \"no value\"\n\t\t}}\n\
                 \t\treturn nil, errors.New(\"{name} <- \" + why)\n\
                 \t}}\n",
                ident = field_pascal(head, config),
                zero = cast_string(&field.target, "\"\""),
                why = why_var(head),
                name = head,
            ));
        } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
            needs_errors = true;
            body.push_str(&format!(
                "\tif len(s.{ident}) == 0 {{\n\
                 \t\twhy := {why}\n\
                 \t\tif why == \"\" {{\n\t\t\twhy = \"no value\"\n\t\t}}\n\
                 \t\treturn nil, errors.New(\"{name} <- \" + why)\n\
                 \t}}\n",
                ident = field_pascal(head, config),
                why = why_var(head),
                name = head,
            ));
        } else if matches!(
            field.target,
            Tref::Prim(
                Prim::I8
                    | Prim::I16
                    | Prim::I32
                    | Prim::I64
                    | Prim::U8
                    | Prim::U16
                    | Prim::U32
                    | Prim::U64
                    | Prim::Float
            )
        ) {
            // A numeric zero can be a legitimate resolved value, so only the
            // combination (chain reported absent, still zero after the
            // bridge) fails construction. A bool has no absent-vs-zero
            // distinction at all, so it carries no require.
            needs_errors = true;
            body.push_str(&format!(
                "\tif {why} != \"\" && s.{ident} == 0 {{\n\
                 \t\treturn nil, errors.New(\"{name} <- \" + {why})\n\
                 \t}}\n",
                ident = field_pascal(head, config),
                why = why_var(head),
                name = head,
            ));
        }
    }
    if needs_errors {
        refs.push(import("errors", "errors"));
    }

    // Declared validation runs last, over what bespoke left in place.
    let mut guards = String::new();
    for field in &entry.fields {
        if field.constraints.is_empty() {
            continue;
        }
        let member = Member {
            name: field.name.clone(),
            target: field.target.clone(),
            required: true,
            default: None,
            constraints: field.constraints.clone(),
            traits: field.traits.clone(),
        };
        let composed = matches!(entry.field_shape(field, module), FieldShape::Config(_));
        for line in validation::guard_lines(&[member], &GoVal, "s.", config, LANG) {
            // The check reads the value bespoke left in place (client_init
            // ran already, bespoke wins), so presence is judged off the value
            // itself, never the declared chain's why-reason.
            let guard = if entry.is_guaranteed(field) || composed {
                line.condition.clone()
            } else if string_like(&field.target) {
                format!(
                    "s.{} != {} && {}",
                    field_pascal(&field.name, config),
                    cast_string(&field.target, "\"\""),
                    line.condition
                )
            } else if matches!(field.target, Tref::Prim(Prim::Bytes)) {
                format!(
                    "len(s.{}) != 0 && {}",
                    field_pascal(&field.name, config),
                    line.condition
                )
            } else if matches!(
                field.target,
                Tref::Prim(
                    Prim::I8
                        | Prim::I16
                        | Prim::I32
                        | Prim::I64
                        | Prim::U8
                        | Prim::U16
                        | Prim::U32
                        | Prim::U64
                        | Prim::Float
                )
            ) {
                // A numeric zero can be a legitimate resolved value, so the
                // check only skips when the chain reported absent AND the
                // bridge left the zero in place (same rule as the requires).
                format!(
                    "({why} == \"\" || s.{ident} != 0) && {}",
                    line.condition,
                    why = why_var(&field.name),
                    ident = field_pascal(&field.name, config),
                )
            } else {
                line.condition.clone()
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
            format!(
                "\t{{\n\t\tms, err := durationMs(string({expr}))\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"{path}: invalid duration %q\", string({expr}))\n\t\t}}\n\t\tvalues[{path:?}] = ms\n\t}}\n",
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

pub(super) fn why_var(field: &str) -> String {
    camel(&format!("{field}_why"))
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
        (None, _) => Some(format!("s.{}", field_pascal(&vp.field.name, config))),
        (Some(member), t) => {
            if matches!(t, Tref::Ref { .. }) && !scalar_ref {
                return None;
            }
            if matches!(t, Tref::Map(_, _) | Tref::List(_)) {
                return None;
            }
            Some(format!(
                "s.{}.{}",
                field_pascal(&vp.field.name, config),
                field_pascal(member, config)
            ))
        }
    }
}

/// Whether a leaf type is an enum reference (a branded string on the wire),
/// which is what lets it freeze into the runtime values.
pub(super) fn ref_is_enum(t: &Tref, module: &Module) -> bool {
    let Tref::Ref { id, .. } = t else {
        return false;
    };
    module
        .shapes
        .iter()
        .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Enum { .. }))
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
/// always frozen. The guard reads the resolved value itself, not the declared
/// chain's why-reason: client_init runs over the Settings before this point
/// (bespoke wins), so a hook-filled field must freeze like a declared one. A
/// non-string value freezes unconditionally (its zero means the same thing to
/// the runtime's value positions as its absence).
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
) -> Option<String> {
    if entry.is_guaranteed(vp.field) && vp.member.is_none() {
        return None;
    }
    if !string_like(vp.target) {
        return None;
    }
    Some(format!("{expr} != {}", cast_string(vp.target, "\"\"")))
}

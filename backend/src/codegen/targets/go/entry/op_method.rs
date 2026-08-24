//! One operation method's whole emission: the zero-value/validate-block
//! prelude, the field-path/timeout-field spellings a call's arguments read
//! off the resolved Settings, and `op_method_decl` itself (the wire call or
//! the bespoke `impl` fallback). Split out of `entry` (this crate's own
//! `mod.rs`) to keep it under the file-size ceiling; `use super::*` reaches
//! the parent's helpers (`discriminator_name`, `entry_field_ident`, the
//! `decode`/`transport`/`ext`/`impl_op` submodules)
//! the same way a sibling of `mod.rs` always has.

use super::*;

/// The zero-value declaration and the return prefix a method needs to bail out
/// early. `var zero T` is the one zero spelling valid for every Go type (a
/// composite literal is not, for primitives). A nullable return's zero is the
/// nil of its pointer (or collection) spelling.
pub(super) fn zero_of(output: Option<&Tref>, nullable: bool) -> (String, &'static str) {
    match output {
        Some(t) => (
            format!("\tvar zero {}\n", go_ret_type(t, nullable)),
            "zero, ",
        ),
        None => (String::new(), ""),
    }
}

/// A constrained input is validated before it leaves the process, so a bad
/// request surfaces as a ValidationError instead of a round trip (or a call into
/// bespoke code that would have to reject it again).
pub(super) fn validate_block(
    input: Option<&Tref>,
    module: &Module,
    ret_zero: &str,
    fail: &dyn Fn(String) -> String,
) -> String {
    let validated = match input {
        Some(Tref::Ref { id, .. }) => module
            .shapes
            .iter()
            .find(|s| s.id == *id)
            .filter(|s| validation::shape_has_checks(s))
            .map(|s| type_ident_from_id(&s.id)),
        _ => None,
    };
    match validated {
        Some(ty) => format!(
            "\tif invalid := Validate{ty}(input); invalid != nil {{\n\t\treturn {ret_zero}{fail_val}\n\t}}\n",
            fail_val = fail("invalid".to_string()),
        ),
        None => String::new(),
    }
}

/// The Go read of a resolved entry-field path off `root`, honoring
/// `@rename(go)` on the leading segment only (a config/struct member's own
/// name is never renamed, only an entry field's is).
pub(super) fn field_path_expr(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    path: &[String],
    root: &str,
) -> String {
    let mut out = root.to_string();
    for (i, seg) in path.iter().enumerate() {
        out.push('.');
        if i == 0 {
            out.push_str(&entry_field_ident(entry, module, config, seg));
        } else {
            out.push_str(&field_pascal(seg, config));
        }
    }
    out
}

/// The unexported client field holding a `@timeout` path's pre-converted
/// `time.Duration`: the path's segments camel-joined, suffixed `Duration`.
/// Collision-free within the client since it derives from the field's own
/// (unique) path.
pub(super) fn timeout_field_ident(
    entry: &EntryModel<'_>,
    _config: &CasingConfig,
    path: &[String],
) -> String {
    let mut out = String::new();
    for (i, seg) in path.iter().enumerate() {
        let rename = if i == 0 {
            entry.field_rename(seg, LANG)
        } else {
            None
        };
        let word = transform(
            seg,
            SymbolKind::Field,
            &CasingConfig::new(CaseStyle::Camel),
            rename.as_deref(),
        );
        if i == 0 {
            out.push_str(&word);
        } else {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    out.push_str("Duration");
    out
}

/// How a resolved field path spells in a wire string position, read off the
/// declared value paths: a string is used verbatim, a branded string flattens
/// through `string(...)`, everything else routes through the shared
/// `FormatScalar`.
fn field_kind_of(target: Option<&Tref>, module: &Module) -> transport::FieldKind {
    match target {
        Some(Tref::Prim(Prim::String | Prim::Uuid)) => transport::FieldKind::StringLike,
        Some(Tref::Prim(Prim::Timestamp | Prim::Date | Prim::Duration)) => {
            transport::FieldKind::Branded
        }
        Some(t @ Tref::Ref { .. }) if ref_is_enum(t, module) => transport::FieldKind::Branded,
        _ => transport::FieldKind::Other,
    }
}

/// One concrete client method: assemble the request from the typed resolved
/// Settings and the encoded input, drive the SDK's emitted transport, and map
/// the raw outcome onto the generated taxonomy.
pub(super) fn op_method_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
) -> Decl {
    let en = error_names();
    let (sig, mut refs) = method_signature(op, config);
    let (input, output) = crate::codegen::ops::op_io(op);
    let output_nullable = crate::codegen::ops::op_output_nullable(op);
    let fail = |expr: String| expr;
    let (zero_decl, ret_zero) = zero_of(output, output_nullable);
    // The zero value and the decode both name the output type in opaque text.
    if let Some(t) = output {
        push_type_symbols(t, &mut refs);
    }
    let validate_block = validate_block(input, module, ret_zero, &fail);
    let Some(wire) = wire_binding(op) else {
        // No protocol binding. Two bespoke shapes reach here: an op's own
        // `impl .field.method(args)` body — a direct call into a
        // declared opaque handle — takes priority when present, otherwise
        // the legacy `ext impl` extension binding the generator gate proved
        // is bound for this target.
        if let Some(call) = crate::codegen::ops::op_impl_call(op) {
            let body = ext::impl_call_body(
                entry,
                module,
                config,
                op_local_name(&op.id),
                crate::codegen::ops::input_name(op),
                call,
                ret_zero,
                &fail,
                &mut refs,
            );
            let doc = doc_of(&op.traits)
                .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
                .unwrap_or_default();
            return Decl::raw_with(
                format!(
                    "{doc}func (c *{client}) {sig} {{\n{zero_decl}{validate_block}{body}}}",
                    client = n.client,
                ),
                refs,
            );
        }
        return impl_op::method_decl(impl_op::Method {
            n,
            op,
            module,
            sig: &sig,
            refs,
            binding: impl_binding(bound, &op.id),
            zero_decl: &zero_decl,
            ret_zero,
            validate_block: &validate_block,
            fail: &fail,
            discriminator: &discriminator_name(n, op),
        });
    };
    let has_declared_errors = !declared_errors(op, module).is_empty();
    let discriminator = discriminator_name(n, op);
    // A wire position reads the typed resolved Settings directly. @timeout is
    // the one exception: its field converts (Duration to time.Duration) rather
    // than passing through, so it reads the unexported, already-converted
    // client field the constructor built.
    let value_paths = entry.value_paths(module);
    let field_access = |path: &[String]| field_path_expr(entry, module, config, path, "c.settings");
    let field_kind = |path: &[String]| {
        let key = path.join(".");
        field_kind_of(
            value_paths
                .iter()
                .find(|vp| vp.path == key)
                .map(|vp| vp.target),
            module,
        )
    };
    // A param-member reference resolves the same way, off the op's own
    // parameter type instead of the entry: a same-module structure member
    // reads as a typed `input.Field`; a cross-module parameter type is not
    // chased here, so the caller falls back to the decoded record.
    let param_access = |seg: &str| -> Option<(String, transport::FieldKind)> {
        let member = crate::codegen::ops::param_member(module, input, seg)?;
        Some((
            field_ident(member, config, LANG),
            field_kind_of(Some(&member.target), module),
        ))
    };
    let success = decode::success_block(
        output,
        output_nullable,
        module,
        &if wire.response_bindings.is_empty() {
            decode::Payload {
                text: "outcome.Body",
                bytes: "[]byte(outcome.Body)",
            }
        } else {
            decode::Payload {
                text: "folded",
                bytes: "[]byte(folded)",
            }
        },
        &fail,
        &mut refs,
    );
    let call = transport::OpCall {
        wire,
        module,
        config,
        has_input: input.is_some(),
        ret_zero,
        discriminator: has_declared_errors.then_some(discriminator.as_str()),
        api_error: &en.api,
        transport_error: &en.transport,
        success_block: &success,
        retry_expr: wire
            .retry
            .as_deref()
            .map(|path| format!("int({})", field_access(path))),
        timeout_expr: wire
            .timeout
            .as_deref()
            .map(|path| format!("c.{}", timeout_field_ident(entry, config, path))),
    };
    let body = transport::op_call(
        &call,
        &fail,
        &field_access,
        &field_kind,
        &param_access,
        &mut refs,
    );
    let doc = doc_of(&op.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let text = format!(
        "{doc}func (c *{client}) {sig} {{\n{zero_decl}{validate_block}{body}}}",
        client = n.client,
    );
    Decl::raw_with(text, refs)
}

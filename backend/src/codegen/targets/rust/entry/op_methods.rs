//! One method per operation on the generated client: a wire-bound op's
//! transport call, an op's own `impl .field.method(args)` body, or a bespoke
//! binding's method. Split out of `mod.rs` to stay under this repo's
//! per-file line ceiling; `op_methods` is `constructor`'s only caller.

use super::*;

/// One async (or sync, per `effect_of`) method per operation.
#[allow(clippy::too_many_arguments)]
pub(super) fn op_methods(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
    timeout_fields: &std::collections::BTreeMap<String, String>,
    refs: &mut Vec<Symbol>,
) -> String {
    let mut out = String::new();
    for op in entry.operations {
        out.push_str(&op_method(
            n,
            op,
            module,
            config,
            entry,
            bound,
            timeout_fields,
            refs,
        ));
        out.push('\n');
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn op_method(
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    entry: &EntryModel<'_>,
    bound: &[BoundExtension<'_>],
    timeout_fields: &std::collections::BTreeMap<String, String>,
    refs: &mut Vec<Symbol>,
) -> String {
    let name = surface::method_name(op, config);
    let (input, output) = op_io(op);
    // A wire-bound op's method carries the transport's own awaits (the send,
    // a retry's backoff sleep), so it is always `async` regardless of what
    // `effect_of` classified it as. Only a bespoke (`impl_op`) method's
    // sync-vs-async follows `effect_of`, since that reflects the bound
    // symbol's own signature.
    let is_async = wire_binding(op).is_some() || effect_of(op) == Effect::Async;

    let (param, input_ty) = match input {
        Some(t) => {
            push_type_symbols(t, &module.name, refs);
            (format!("input: {}", rust_type(t)), Some(rust_type(t)))
        }
        None => (String::new(), None),
    };
    if let Some(t) = output {
        push_type_symbols(t, &module.name, refs);
    }
    let output_nullable = crate::codegen::ops::op_output_nullable(op);
    let ret = output
        .map(|t| {
            let base = rust_type(t);
            if output_nullable {
                format!("Option<{base}>")
            } else {
                base
            }
        })
        .unwrap_or_else(|| "()".into());

    let mut validate_block = String::new();
    if let Some(Tref::Ref { id, .. }) = input {
        if module
            .shapes
            .iter()
            .find(|s| s.id == *id)
            .is_some_and(validation::shape_has_checks)
        {
            validate_block = "if let Err(e) = input.validate() {\n    return Err(TonoError::Validation(e));\n}\n".to_string();
        }
    }

    let doc = doc_of(&op.traits)
        .map(|d| crate::codegen::doc::rustdoc(&d, "    "))
        .unwrap_or_default();

    let Some(wire) = wire_binding(op) else {
        // No protocol binding. An op's own `impl .field.method(args)` body (a
        // call straight into a declared opaque handle) takes priority when
        // present, matching Go's and TypeScript's own dispatch order;
        // otherwise the operation is implemented by bespoke sources the
        // generator gate proved are bound for this target.
        if let Some(call) = crate::codegen::ops::op_impl_call(op) {
            let (body, call_is_async) = ext::impl_call_body(&ext::ImplCall {
                entry,
                module,
                config,
                call,
                input_name: crate::codegen::ops::input_name(op),
                has_output: output.is_some(),
            });
            // A handle call's own method follows the extern's `sync` flag,
            // the same way a free extern call's `.await` does: there is no
            // transport here to make the method async on its own.
            let effect = if call_is_async { "async " } else { "" };
            return format!(
                "{doc}    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n{validate_block}{body}\n    }}",
                comma = if param.is_empty() { "" } else { ", " },
                validate_block = indent(&validate_block, 2),
                body = indent(&body, 2).trim_end_matches('\n'),
            );
        }
        let discriminator_name = surface::discriminator_fn_name(n, op);
        let discriminator =
            (!declared_errors(op, module).is_empty()).then_some(discriminator_name.as_str());
        return impl_op::method(impl_op::Method {
            op,
            module,
            name: &name,
            param: &param,
            ret: &ret,
            input_ty: input_ty.as_deref(),
            output,
            discriminator,
            validate_block: &validate_block,
            binding: impl_binding(bound, &op.id),
            is_async,
            doc: &doc,
            refs,
        });
    };

    let discriminator = surface::discriminator_fn_name(n, op);
    let success = decode::success_block(output, output_nullable, module, "&outcome.body");
    let fields = transport::FieldCtx {
        entry,
        module,
        config,
        input,
    };
    // The already-converted milliseconds field for the op's `@timeout` path,
    // built by the constructor (see `constructor::timeout_conversions`).
    let timeout_field = wire
        .timeout
        .as_ref()
        .map(|path| timeout_fields[&path.join(".")].clone());
    let body = transport::op_call(
        &transport::OpCall {
            wire,
            method: &wire.method,
            has_input: input.is_some(),
            has_declared_errors: !declared_errors(op, module).is_empty(),
            discriminator: &discriminator,
            success_block: &success,
            timeout_field,
        },
        &fields,
        refs,
    );
    format!(
        "{doc}    pub async fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n{validate_block}{body}    }}",
        comma = if param.is_empty() { "" } else { ", " },
        validate_block = indent(&validate_block, 2),
        body = indent(&body, 2),
    )
}

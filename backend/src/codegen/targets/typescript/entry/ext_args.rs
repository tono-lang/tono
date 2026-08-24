//! Rendering one node of a `ts` language block's `call_args` template as a
//! TypeScript expression: the pure, recursive part of emitting one extern
//! call, shared by a free extern-fn field's own construction call and an
//! op's `impl .field.method(args)` body (`ext_call`/`ext_handle_call`).
//! Split out of `ext_call` to keep it under the file-size ceiling; the
//! callee-side pieces (the seam, `spell`, imports, projections) stay there.

use super::checks::field_path_expr;
use super::ext_call::{class_reference_name, spell};
use super::ext_coerce;
use crate::codegen::entries::EntryModel;
use crate::ir::{CallArg, CallCtor, ExtLib, ExternParam, Module};

pub(super) fn json_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Array(items) => {
            format!(
                "[{}]",
                items
                    .iter()
                    .map(json_literal)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        serde_json::Value::Object(map) => format!(
            "{{ {} }}",
            map.iter()
                .map(|(k, v)| format!("{k}: {}", json_literal(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// [`render_arg`]'s own default `Ref` resolution: a sibling entry field read
/// off `s`, the seam function's own parameter. A handle-method call site
/// (`ext_handle_call.rs`) reads off a different root and also recognizes the
/// op's own input parameter, so that call site supplies its own resolver
/// instead of this default.
pub(super) fn field_ref(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    path: &[String],
) -> String {
    field_path_expr(entry, config, path, "s")
}

/// Render one node of a `ts` language block's `call_args` template, purely
/// from (entry, config): `Resolver::path_read` is itself a pure function of
/// exactly those two things (`field_path_expr`), so this needs no `Resolver`
/// to reach byte-identical output. `Param` substitutes the extern's declared
/// logical parameter with the value the entry field's own call site passed
/// for it (positional against `params`); the substituted value is rendered
/// with an empty `(params, site_args)`, so a stray `Param` inside it (which
/// the grammar never produces) fails loudly instead of silently matching the
/// outer template's parameters. `ref_expr` resolves a `Ref` leaf: the two
/// call sites (a free extern-fn field and an op's own handle-method call)
/// read a `Ref`'s root differently, so this is the one seam they don't share.
#[allow(clippy::too_many_arguments)]
pub(super) fn render_arg(
    entry: &EntryModel<'_>,
    config: &crate::codegen::casing::CasingConfig,
    module: &Module,
    lib: &ExtLib,
    arg: &CallArg,
    params: &[ExternParam],
    site_args: &[CallArg],
    ref_expr: &dyn Fn(&EntryModel<'_>, &crate::codegen::casing::CasingConfig, &[String]) -> String,
) -> String {
    match arg {
        // The parameter crosses under its own TypeScript spelling: the
        // caller's actual argument, converted when the spelling sits on
        // the other side of the number/bigint divide (`port: #(number)`
        // on an i64). Any other spelling is structural and the value
        // passes as it is, for the compiler to grade against the
        // library's own declaration (see `ext_coerce`).
        CallArg::ParamAs { name, spelling } => {
            let param = params.iter().find(|p| &p.name == name).unwrap_or_else(|| {
                panic!("extern call template references undeclared parameter {name:?}")
            });
            let value = render_arg(
                entry,
                config,
                module,
                lib,
                &CallArg::Param(name.clone()),
                params,
                site_args,
                ref_expr,
            );
            ext_coerce::coerce(&param.r#type, spelling, &value).unwrap_or_else(|e| {
                panic!("{e}; validate_calls::param_spellings_coerce should have refused it")
            })
        }
        // TypeScript binds no position of its own: `validate_calls::
        // foreign_position_binds` refuses a declared position here.
        CallArg::Foreign(sp) => panic!(
            "a declared position {sp:?} reached TypeScript codegen; validate_calls::foreign_position_binds should have refused it"
        ),
        CallArg::Param(name) => {
            let idx = params
                .iter()
                .position(|p| &p.name == name)
                .unwrap_or_else(|| {
                    panic!("extern call template references undeclared parameter {name:?}")
                });
            let site = site_args.get(idx).unwrap_or_else(|| {
                panic!("extern call site is missing an argument for parameter {name:?}")
            });
            render_arg(entry, config, module, lib, site, &[], &[], ref_expr)
        }
        CallArg::Ref(path) => ref_expr(entry, config, path),
        CallArg::Lit(v) => json_literal(v),
        CallArg::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| render_arg(entry, config, module, lib, i, params, site_args, ref_expr))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // A foreign struct literal: each field's value converted when the
        // form's ts block spells the field under its own type, the same
        // rule a spelled parameter follows.
        CallArg::Ctor(CallCtor { name, fields }) => {
            let form = lib.structs.iter().find(|s| s.name == *name);
            let block = form.and_then(|f| {
                f.langs
                    .iter()
                    .find(|l| l.lang == "ts" || l.lang == "typescript")
            });
            let rendered = fields
                .iter()
                .map(|(k, v)| {
                    let value =
                        render_arg(entry, config, module, lib, v, params, site_args, ref_expr);
                    let value = match (
                        block.and_then(|b| b.fields.get(k)),
                        form.and_then(|f| f.fields.iter().find(|ff| &ff.name == k)),
                    ) {
                        (Some(spelling), Some(ff)) => {
                            ext_coerce::coerce(&ff.r#type, spelling, &value).unwrap_or_else(|e| {
                                panic!("{e}; validate_calls::foreign_forms_declared should have refused it")
                            })
                        }
                        _ => value,
                    };
                    format!("{k}: {value}")
                })
                .collect::<Vec<_>>();
            format!("{{ {} }}", rendered.join(", "))
        }
        // A cross-extern call as a call argument (`ns.fn(...)` naming a
        // *declared* extern, reached today only from a ctor field's own
        // value): rejected before generation reaches here, see the module
        // doc comment in `ext_call`.
        CallArg::Call(_) => {
            panic!(
                "a cross-extern call as a call argument reached TypeScript codegen; \
                 validate_calls::extern_binds_every_target should have rejected it first"
            )
        }
        // A bare foreign-symbol call nested inside a `call:` line's own
        // argument list, e.g. `WithPrecision(precision)`: no declared
        // extern to resolve against, so no yields/returns/errors
        // projection, no `await` -- rendered as a plain synchronous,
        // infallible call, the shape the bench's own nested calls have.
        CallArg::SymbolCall(sc) => format!(
            "{}({})",
            spell(&sc.symbol, module),
            sc.args
                .iter()
                .map(|a| render_arg(entry, config, module, lib, a, params, site_args, ref_expr))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        // A class reference: the declared handle's own class, passed as the
        // imported identifier (the library constructs it; tono only names
        // it). The import itself is collected by `class_reference_imports`
        // at the call site that owns the seam's refs.
        CallArg::TypeRef(handle) => class_reference_name(lib, handle),
    }
}

//! The TypeScript entry client: the SDK construction surface an entry
//! declares, spelled as an exported class per entry (it replaces the generic
//! `HttpClient` for a module that declares one). The constructor takes the
//! `@arg` fields positionally and the `@with` fields as an optional config
//! object, resolves every declared source chain top-down, parses env values by
//! the field's type at the boundary (the error names the variable and the
//! type), lowers a match selection to a `switch`, decodes a structured source
//! strictly, composes a config through `@bind`, runs the bound `client_init`
//! hook over the resolved `Settings` (bespoke wins, by mutation), and
//! validates last. Each operation method resolves its own `wire` binding's
//! ref positions (endpoint, headers, timeout, retry) against the resolved
//! fields directly and builds the request inline; no runtime interprets a
//! descriptor.

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    deprecated_of, doc_of, field_ident, rename_of, type_ident_from_id, wire_key,
};
use crate::codegen::entries::{companion_name, op_local_name, plan, EntryModel, FieldShape};
use crate::codegen::extensions::{hook_binding, impl_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_binding};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{EntryField, EnvName, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Tref};

const BINDING_LANGS: [&str; 2] = ["ts", "typescript"];

fn pascal(name: &str) -> String {
    transform(
        name,
        SymbolKind::Type,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    )
}

fn camel(name: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Camel),
        None,
    )
}

struct Names {
    client: String,
    settings: String,
    config: String,
    op_prefix: String,
}

fn names(entry: &EntryModel<'_>, multi: bool) -> Names {
    Names {
        client: pascal(entry.name),
        settings: pascal(&companion_name(entry.name, "settings", multi)),
        config: pascal(&format!("{}_config", entry.name)),
        op_prefix: if multi {
            format!("{}_", entry.name)
        } else {
            String::new()
        },
    }
}

fn ts_type(t: &Tref) -> String {
    render_type(&type_expr_of(t), &TsRules)
}

fn field_camel(name: &str, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, None)
}

/// An entry field's public identifier, honoring its `@rename(typescript)`
/// override. Config members and path tails keep the plain [`field_camel`].
fn field_camel_ren(name: &str, rename: Option<&str>, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, rename)
}

/// The JSDoc `@doc`/`@deprecated` block for an entry field's public surface (a
/// Settings or config-object property), indented and newline-terminated, or
/// empty when the field carries neither trait.
fn field_doc(traits: &[crate::ir::Trait], indent: &str) -> String {
    crate::codegen::doc::jsdoc_block(
        doc_of(traits).as_deref(),
        deprecated_of(traits).as_deref(),
        indent,
    )
}

/// A reference to a declaration of the SDK's shared support group.
fn support_symbol(name: &str) -> Symbol {
    Symbol::imported(name, crate::codegen::group::ROOT_SUPPORT, name)
}

pub(super) fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// Whether `t` names an opaque handle declared in one of the module's own
/// `ext` blocks: never serializes, never crosses the wire, and
/// has no companion import (its only real type lives in the third-party
/// module the SDK never re-exports). Rendered as `unknown` everywhere a
/// declared type is spelled, since the SDK only ever passes the handle
/// around, never reads it.
fn foreign_handle(t: &Tref, module: &Module) -> bool {
    matches!(t, Tref::Ref { id, .. } if crate::codegen::entries::is_foreign_ref(module, id))
}

/// A field's declared type, rendered for the generated surface: `unknown`
/// for a foreign opaque handle (see [`foreign_handle`]), the ordinary
/// [`ts_type`] spelling otherwise.
fn field_ts_type(t: &Tref, module: &Module) -> String {
    if foreign_handle(t, module) {
        "unknown".to_string()
    } else {
        ts_type(t)
    }
}

/// The companion-file symbols a declared type drags into the serde file: the
/// branded well-known aliases and any named shape.
fn type_refs(t: &Tref, module: &Module) -> Vec<Symbol> {
    match t {
        // A branded well-known type is the SDK's, not the module's.
        Tref::Prim(Prim::Timestamp) => vec![support_symbol("Timestamp")],
        Tref::Prim(Prim::Date) => vec![support_symbol("LocalDate")],
        Tref::Prim(Prim::Duration) => vec![support_symbol("Duration")],
        Tref::Ref { id, .. } if foreign_handle(t, module) => {
            let _ = id;
            Vec::new()
        }
        Tref::Ref { id, .. } => {
            // A config interface lives in this same serde file (it is part of
            // the entry surface), so it needs no companion import.
            let is_config = module
                .shapes
                .iter()
                .any(|shape| shape.id == *id && matches!(shape.kind, ShapeKind::Config { .. }));
            if is_config {
                Vec::new()
            } else {
                vec![module_symbol(&type_ident_from_id(id), module)]
            }
        }
        Tref::List(inner) => type_refs(inner, module),
        Tref::Map(k, v) => {
            let mut out = type_refs(k, module);
            out.extend(type_refs(v, module));
            out
        }
        _ => Vec::new(),
    }
}

/// The zero value the mutable Settings draft starts from, per declared type.
fn zero_value(t: &Tref, module: &Module) -> String {
    match t {
        Tref::Prim(Prim::Bool) => "false".into(),
        Tref::Prim(Prim::I64 | Prim::U64) => "0n".into(),
        Tref::Prim(
            Prim::I8 | Prim::I16 | Prim::I32 | Prim::U8 | Prim::U16 | Prim::U32 | Prim::Float,
        ) => "0".into(),
        Tref::Prim(Prim::String | Prim::Uuid) => "\"\"".into(),
        Tref::Prim(Prim::Bytes) => "new Uint8Array()".into(),
        Tref::Map(_, _) => "{}".into(),
        Tref::List(_) => "[]".into(),
        // A foreign handle has no shape to start from; the call the field
        // also declares is what actually produces it (see `ext_call.rs`),
        // so the draft only needs a placeholder `unknown` accepts.
        Tref::Ref { .. } if foreign_handle(t, module) => "undefined".into(),
        // A named shape starts from an empty object (mirroring Go's zero
        // struct); only string-branded leaves start from the empty string.
        Tref::Ref { .. } => format!("{{}} as {}", ts_type(t)),
        _ => format!("\"\" as {}", ts_type(t)),
    }
}

fn cast_string(t: &Tref, v: &str) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => v.to_string(),
        _ => format!("{v} as {}", ts_type(t)),
    }
}

/// A resolved field rendered as a string for template concatenation.
fn as_template_string(expr: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        _ => format!("String({expr})"),
    }
}

fn literal(t: &Tref, v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => cast_string(t, &format!("{s:?}")),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => match t {
            Tref::Prim(Prim::I64 | Prim::U64) => format!("{n}n"),
            _ => n.to_string(),
        },
        other => format!("{other}"),
    }
}

fn pattern_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        other => format!("{other}"),
    }
}

#[derive(Default)]
struct Helpers {
    read_env: bool,
    duration_ms: bool,
    transforms: BTreeSet<&'static str>,
    /// Extra import symbols an extern-call leaf needs in its class's own
    /// file (the foreign function, the sentinel error classes it throws):
    /// [`plan::Emitter::call_assign`] has no other way to hand a `Symbol`
    /// back up to the `refs` the enclosing `Decl` carries.
    ext_refs: Vec<Symbol>,
    /// Every distinct sentinel-mapped type name (`ErrorBinding.r#type`) any
    /// extern-call leaf in the module declared, so `emit` can generate each
    /// class once, module-wide, after every entry has been built.
    ext_error_types: BTreeSet<String>,
}

pub use plan::EntryEmission;

/// Emit a module's entries: the Settings and config interfaces, the descriptor
/// constants, the class and its methods, grouped per entry so the construction
/// surface reads together; the wrappers and helpers every entry shares stay in
/// the module's internal group, emitted once.
pub fn emit(module: &Module, config: &CasingConfig) -> EntryEmission {
    let Some((entries, multi, bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
        return EntryEmission::empty();
    };
    let tested = crate::codegen::declared_tests::entries_with_tests(module);
    let mut helpers = Helpers::default();
    let mut decls = Vec::new();
    decls.extend(config_interfaces(module, config));
    decls.extend(transport_hook_wrappers(&bound, module));
    // A bound contract/constraint gets its boundary wrapper here too, or the
    // binding would go silently unused.
    let mut contract_refs = Vec::new();
    let contracts = crate::codegen::targets::typescript::client::contract_wrappers(
        &bound,
        module,
        &mut contract_refs,
    );
    if !contracts.is_empty() {
        decls.push(Decl::raw_with(contracts, contract_refs));
    }
    decls.extend(client_init_wrapper(&bound, &entries, multi, module));
    decls.extend(plan::output_decode_decls(
        &entries,
        module,
        |op| wire_binding(op).is_some() || impl_binding(&bound, &op.id).is_some_and(|b| b.raw),
        |shape| decode::output_decode_decl(shape, module),
    ));
    let mut per_entry = Vec::new();
    for entry in &entries {
        let n = names(entry, multi);
        let has_tests = tested.contains(entry.name);
        let mut own = vec![settings_interface(entry, &n, config, module)];
        own.extend(config_object_interface(entry, &n, config, module));
        own.extend(impl_op::seam_decls(entry, &n, module, &bound, has_tests));
        own.extend(class_decl(
            entry,
            &n,
            module,
            config,
            &bound,
            &mut helpers,
            multi,
            has_tests,
        ));
        own.extend(discriminator_decls_for(entry, &n, module, &bound));
        per_entry.push((entry.name.to_string(), own));
    }
    // One class per distinct sentinel-mapped type name any entry's extern
    // call declared, module-wide rather than per entry: two entries mapping
    // the same sentinel share the one class.
    for sentinel_type in &helpers.ext_error_types {
        decls.push(ext_call::sentinel_error_decl(sentinel_type, module));
    }
    EntryEmission {
        shared: decls,
        per_entry,
    }
}

/// One operation method: builds the request from the resolved wire binding,
/// calls the transport inline, and maps the outcome onto the taxonomy.
#[allow(clippy::too_many_arguments)]
fn op_method(
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    entry: &EntryModel<'_>,
    bound: &[BoundExtension<'_>],
    timeout_field_expr: &dyn Fn(&[String]) -> String,
    refs: &mut Vec<Symbol>,
) -> String {
    let en = error_names();
    let name = method_name(op, config);
    let (input, output) = crate::codegen::ops::op_io(op);
    let has_on_error = hook_binding(bound, "on_error").is_some();
    let throw = |expr: String| {
        if has_on_error {
            format!("throw wrapOnError({expr});")
        } else {
            format!("throw {expr};")
        }
    };

    let (param, input_expr) = match input {
        Some(t) => {
            refs.extend(type_refs(t, module));
            let ty = render_type(&type_expr_of(t), &TsRules);
            let expr = match t {
                Tref::Ref { id, .. } => {
                    format!("encode{}(input)", type_ident_from_id(id))
                }
                _ => "input".to_string(),
            };
            (format!("input: {ty}"), expr)
        }
        None => (String::new(), "{}".to_string()),
    };
    if let Some(t) = output {
        refs.extend(type_refs(t, module));
    }
    let ret = output
        .map(|t| render_type(&type_expr_of(t), &TsRules))
        .unwrap_or_else(|| "void".to_string());

    // A constrained input is validated before it leaves the process, so a bad
    // request surfaces as a ValidationError instead of a server round trip (or
    // a call into bespoke code that would have to reject it again).
    let mut validate_block = String::new();
    if let Some(Tref::Ref { id, .. }) = input {
        if let Some(shape) = module
            .shapes
            .iter()
            .find(|s| s.id == *id)
            .filter(|s| validation::shape_has_checks(s))
        {
            let ty = type_ident_from_id(&shape.id);
            refs.push(module_symbol(&format!("validate{ty}"), module));
            validate_block = format!(
                "    const invalid = validate{ty}(input);\n    if (invalid) {{\n      {t}\n    }}\n",
                t = throw("invalid".to_string()),
            );
        }
    }

    let Some(wire) = wire_binding(op) else {
        // No protocol binding: the operation is implemented by bespoke sources
        // the frontend proved are bound, and the generator gate proved are bound
        // for this target.
        return impl_op::method(impl_op::Method {
            n,
            op,
            module,
            name: &name,
            param: &param,
            ret: &ret,
            input_expr: &input_expr,
            output,
            validate_block: &validate_block,
            binding: impl_binding(bound, &op.id),
            throw: &throw,
            discriminator: &discriminator_name(n, op),
            refs,
        });
    };

    let has_declared_errors = !declared_errors(op, module).is_empty();
    let discriminator = discriminator_name(n, op);
    let error_line = if has_declared_errors {
        throw(format!("{discriminator}(outcome.status, outcome.body)"))
    } else {
        refs.push(module_symbol(&en.api, module));
        throw(format!("new {}(outcome.status, outcome.body)", en.api))
    };
    let success_block = decode::success_block(output, module, &ret, &throw, refs);
    let before_request_bound = hook_binding(bound, "before_request").is_some();
    let after_response_bound = hook_binding(bound, "after_response").is_some();
    let http_method = wire.method.clone();
    // The frontend guarantees an endpoint/@header/path-template/@retry field
    // reference resolves to a value already sitting, typed, on the resolved
    // Settings — read it there directly instead of through a runtime bag.
    // @timeout is the one exception: its field converts (Duration to
    // milliseconds) rather than passing through, so it reads the private,
    // already-converted field the constructor built (see class_decl).
    let field_expr = |path: &[String]| field_path_expr(entry, config, path, "this.settings");
    // A param-member reference resolves the same way, off the op's own
    // parameter type instead of the entry: a same-module structure member
    // reads as a typed `input.field`; a cross-module parameter type is not
    // chased here, so the caller falls back to the decoded record.
    let param_access = |seg: &str| -> Option<String> {
        let member = crate::codegen::ops::param_member(module, input, seg)?;
        Some(format!("input.{}", field_ident(member, config, LANG)))
    };
    let transport_body = transport::op_call(
        wire,
        &http_method,
        &input_expr,
        has_declared_errors,
        &discriminator,
        &error_line,
        &success_block,
        &en.transport,
        &throw,
        before_request_bound,
        after_response_bound,
        &field_expr,
        timeout_field_expr,
        &param_access,
        refs,
    );
    let doc = doc_of(&op.traits)
        .map(|d| format!("  // {}\n", d.replace('\n', "\n  // ")))
        .unwrap_or_default();
    format!(
        "{doc}  async {name}({param}): Promise<{ret}> {{\n\
         {validate_block}\
         {transport_body}\
         \x20 }}",
    )
}

use plan::err_var;

mod checks;
mod class;
mod decode;
mod ext_call;
mod impl_op;
mod resolve;
mod surface;
#[cfg(test)]
mod tests;
pub(crate) mod transport;
pub(crate) mod transport_decls;
pub(crate) mod vector_tests;

use checks::{access, config_error, field_path_expr, presence_guard, timeout_field_name};
use class::class_decl;
use resolve::Resolver;
use surface::*;
pub use surface::{casing_helpers, duration_helpers, env_helpers, resolution_helpers};

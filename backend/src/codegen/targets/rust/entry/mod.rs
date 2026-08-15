//! The Rust entry client: the SDK construction surface an entry declares,
//! spelled as idiomatic async Rust carrying its own inline HTTP transport
//! ([`transport`]). A `@with`-bearing entry gets a builder
//! (`Client::builder(<@arg fields>) -> ClientBuilder`, `.with_<field>(v)` per
//! `@with` field, `.build(self) -> Result<Client, TonoError>`); an entry with
//! no `@with` fields gets a plain `Client::new(<@arg fields>) -> Result<Self,
//! TonoError>` instead — a builder with nothing to configure is just a
//! function. Either way the body resolves every declared source chain
//! top-down (explicit value wins over the chain, `@default` is the last
//! resort), parses env values by the field's type at the boundary, lowers a
//! match selection to Rust's own `match`, decodes a structured source
//! strictly, composes a config through `@bind`, runs the bound `client_init`
//! hook over the resolved `Settings` (bespoke wins), validates last, and
//! freezes the transport slots into `ClientOptions`. An operation's wire
//! positions (endpoint, headers, timeout, retry) read the typed resolved
//! `Settings` directly at the call site — there is no runtime value bag and
//! no descriptor left to interpret.
//!
//! Each entry declaration lands in its own emission group, named after it
//! (`Group::entry`, [`emit`]'s `per_entry`): the whole surface
//! (`Settings`/`Builder`/`Client`/its op methods/its discriminators)
//! references only itself, the module's own shared config structs, external
//! crates (`serde_json`, `tono_http_runtime`, `std`), and the SDK-root
//! resolution helpers ([`shared`]), so one file per entry reads together
//! instead of being split across a types and a codec file (mirrors Go's
//! `targets/go/entry::emit`).

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{deprecated_of, doc_of, rename_of, type_ident_from_id, wire_key};
use crate::codegen::entries::{companion_name, op_local_name, plan, EntryModel};
use crate::codegen::extensions::{hook_binding, impl_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, effect_of, op_io, wire_binding, Effect};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::rust::render::RustRules;
use crate::codegen::targets::rust::types::{type_expr_of, variant_ident, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{
    EntryField, EnumBacking, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Trait, Tref,
};

const BINDING_LANGS: [&str; 1] = ["rust"];

fn pascal(name: &str) -> String {
    transform(
        name,
        SymbolKind::Type,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    )
}

/// The canonical-name-to-identifier casing every generated Rust local/field
/// uses: snake_case, matching [`crate::codegen::targets::rust::types::rust_casing`].
/// The shared plan's own `camel`/`arg_camel` helpers hard-code camelCase
/// (right for Go/TypeScript locals, wrong for Rust — `non_snake_case` would
/// fail this crate's own `-D warnings` gate on a generated SDK), so every
/// identifier the Rust target spells goes through this instead.
pub(super) fn snake(name: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Snake),
        None,
    )
}

pub(super) fn arg_snake(name: &str, traits: &[Trait], lang: &str) -> String {
    transform(
        name,
        SymbolKind::Field,
        &CasingConfig::new(CaseStyle::Snake),
        rename_of(traits, lang).as_deref(),
    )
}

pub(super) fn err_var(field: &str) -> String {
    snake(&format!("{field}_err"))
}

pub(super) fn field_snake(name: &str, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, None)
}

pub(super) fn field_snake_ren(name: &str, rename: Option<&str>, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, rename)
}

pub(super) fn rust_type(t: &Tref) -> String {
    // An entry's construction surface always rides the module's public
    // group, so its types render with `pub` visibility (`crate_visible:
    // false`), never `pub(crate)`.
    render_type(&type_expr_of(t), &RustRules::default())
}

/// The symbols a declared type references, for import collection off a raw
/// declaration. Every entry group already carries a glob `use` of its own
/// module's public types ([`emit`]'s `types_glob_use`), so a same-module
/// reference needs no ref of its own here — pushing one anyway would render
/// as a second, redundant `use` naming exactly what the glob already brings
/// in; only an actually cross-module reference produces a real import.
pub(super) fn push_type_symbols(t: &Tref, module_name: &str, refs: &mut Vec<Symbol>) {
    let mut collected = Vec::new();
    type_expr_of(t).flatten_symbols(&mut collected);
    refs.extend(collected.into_iter().filter(|s| match &s.import {
        Some(i) => i.module != module_name,
        None => true,
    }));
}

/// The per-entry generated names, derived once.
pub(super) struct Names {
    pub client: String,
    pub settings: String,
    pub builder: String,
    /// The canonical prefix for per-op companions (descriptor fns,
    /// discriminators): empty for a single entry.
    pub op_prefix: String,
}

pub(super) fn names(entry: &EntryModel<'_>, multi: bool) -> Names {
    let client = pascal(entry.name);
    Names {
        settings: pascal(&companion_name(entry.name, "settings", multi)),
        builder: format!("{client}Builder"),
        client,
        op_prefix: if multi {
            format!("{}_", entry.name)
        } else {
            String::new()
        },
    }
}

/// The zero value the mutable `Settings`/config draft starts from, per
/// declared type: Go/TypeScript's "zero struct" semantics, ported as closely
/// as a real (non-nullable) Rust struct field allows. A named shape
/// recurses into its own zeroed literal (a config composes its members the
/// same way; a structure zeroes its required members and leaves its
/// optional ones `None`); an enum starts at its `Unknown` catch-all over an
/// empty backing value, which is a real, valid instance (never a
/// `Default`/`unreachable!` placeholder) so a field that nothing resolves
/// keeps exactly this value, precisely like Go's zero struct / TypeScript's
/// `{}` cast.
pub(super) fn zero_value(t: &Tref, module: &Module, config: &CasingConfig) -> String {
    match t {
        Tref::Prim(Prim::Bool) => "false".into(),
        Tref::Prim(
            Prim::I8
            | Prim::I16
            | Prim::I32
            | Prim::I64
            | Prim::U8
            | Prim::U16
            | Prim::U32
            | Prim::U64,
        ) => "0".into(),
        Tref::Prim(Prim::Float) => "0.0".into(),
        Tref::Prim(Prim::String | Prim::Uuid) => "String::new()".into(),
        Tref::Prim(Prim::Bytes) => "Vec::new()".into(),
        Tref::Prim(Prim::Timestamp) => "Timestamp(String::new())".into(),
        Tref::Prim(Prim::Date) => "LocalDate(String::new())".into(),
        Tref::Prim(Prim::Duration) => "Duration(String::new())".into(),
        Tref::List(_) => "Vec::new()".into(),
        Tref::Map(_, _) => "std::collections::HashMap::new()".into(),
        Tref::Param(_) => "Default::default()".into(),
        Tref::Ref { id, .. } => zero_ref(id, module, config),
    }
}

fn zero_ref(id: &str, module: &Module, config: &CasingConfig) -> String {
    let Some(shape) = module.shapes.iter().find(|s| s.id == *id) else {
        // The entry-surface validator (`entries::validate_entries`) already
        // rejects a construction field referencing a shape outside its
        // module, so every real id resolves here; kept total anyway.
        return "unreachable!(\"entry field type resolves outside its module\")".into();
    };
    let name = type_ident_from_id(id);
    match &shape.kind {
        ShapeKind::Config { fields } => {
            let members: String = fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}, ",
                        field_snake(&f.name, config),
                        zero_value(&f.target, module, config)
                    )
                })
                .collect();
            format!("{name} {{ {members}}}")
        }
        ShapeKind::Structure { members, .. } => {
            let fields: String = members
                .iter()
                .map(|m| {
                    let ident = field_snake(&m.name, config);
                    if m.required {
                        format!("{ident}: {}, ", zero_value(&m.target, module, config))
                    } else {
                        format!("{ident}: None, ")
                    }
                })
                .collect();
            format!("{name} {{ {fields}}}")
        }
        ShapeKind::Enum { backing, .. } => match backing {
            EnumBacking::String => format!("{name}::Unknown(String::new())"),
            EnumBacking::Int => format!("{name}::Unknown(0)"),
        },
        // A union, service, entry, or operation target is not a
        // construction shape (no realistic entry declares one as a field
        // type); defensively unreachable rather than emitting text that
        // would not compile.
        _ => format!("unreachable!(\"{name} is not a constructible entry field shape\")"),
    }
}

/// The Rust literal for a `@default`/match-arm JSON value of the given type.
pub(super) fn literal(t: &Tref, v: &serde_json::Value, module: &Module) -> String {
    match t {
        Tref::Prim(Prim::Bool) => v.as_bool().unwrap_or(false).to_string(),
        Tref::Prim(
            Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64 | Prim::U8 | Prim::U16 | Prim::U32,
        ) => v
            .as_i64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "0".into()),
        Tref::Prim(Prim::U64) => v
            .as_u64()
            .map(|n| n.to_string())
            .unwrap_or_else(|| "0".into()),
        Tref::Prim(Prim::Float) => v
            .as_f64()
            .map(|n| format!("{n}f64"))
            .unwrap_or_else(|| "0.0".into()),
        Tref::Prim(Prim::String | Prim::Uuid) => {
            format!("{:?}.to_string()", v.as_str().unwrap_or_default())
        }
        Tref::Prim(Prim::Timestamp) => format!(
            "Timestamp({:?}.to_string())",
            v.as_str().unwrap_or_default()
        ),
        Tref::Prim(Prim::Date) => format!(
            "LocalDate({:?}.to_string())",
            v.as_str().unwrap_or_default()
        ),
        Tref::Prim(Prim::Duration) => {
            format!("Duration({:?}.to_string())", v.as_str().unwrap_or_default())
        }
        Tref::Prim(Prim::Bytes) => "Vec::new()".into(),
        Tref::Ref { id, .. } => literal_enum(id, v, module),
        _ => "Default::default()".into(),
    }
}

/// A `@default`/match-arm value targeting an enum ref: resolved to its known
/// variant when the JSON literal matches a declared wire value, else its
/// `Unknown` catch-all (an open enum accepts any wire value, declared or
/// not).
fn literal_enum(id: &str, v: &serde_json::Value, module: &Module) -> String {
    let name = type_ident_from_id(id);
    let variant_config = CasingConfig::new(CaseStyle::Pascal);
    let Some(shape) = module.shapes.iter().find(|s| s.id == *id) else {
        return format!("{name}::Unknown(String::new())");
    };
    if let ShapeKind::Enum { backing, values } = &shape.kind {
        return match backing {
            EnumBacking::String => {
                let s = v.as_str().unwrap_or_default();
                match values.iter().find(|ev| ev.name == s) {
                    Some(known) => {
                        format!("{name}::{}", variant_ident(&known.name, &variant_config))
                    }
                    None => format!("{name}::Unknown({s:?}.to_string())"),
                }
            }
            EnumBacking::Int => {
                let n = v.as_i64().unwrap_or_default();
                match values.iter().find(|ev| ev.value == Some(n)) {
                    Some(known) => {
                        format!("{name}::{}", variant_ident(&known.name, &variant_config))
                    }
                    None => format!("{name}::Unknown({n})"),
                }
            }
        };
    }
    format!("{name}::Unknown(String::new())")
}

/// A match-arm pattern rendered against the Display-stringified subject (see
/// [`resolve::Resolver::switch_header`]): every pattern kind reduces to the
/// same `&str` comparison, sidestepping Rust's per-type match-pattern syntax
/// (a `String` subject cannot pattern-match a bare string literal, an enum
/// has no literal pattern form at all) without needing type information at
/// the call site.
pub(super) fn pattern_literal(pattern: &serde_json::Value) -> String {
    let s = match pattern {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    format!("{s:?}")
}

/// The numeric literal `0` reads correctly for every integer width by
/// inference; a float needs the explicit `.0` or Rust rejects the bare
/// integer literal in a `f64` position.
pub(super) fn numeric_zero(t: &Tref) -> &'static str {
    if matches!(t, Tref::Prim(Prim::Float)) {
        "0.0"
    } else {
        "0"
    }
}

/// Which on-demand casing-transform functions the emitted code pulled in
/// ([`apply_transforms`]'s own bookkeeping for its `helper` closure); every
/// other shared resolution helper ([`shared_symbol`]/[`shared_slot`]) is
/// tracked directly as a `Symbol` ref instead, since that is what drives the
/// assembler's real import collection and pruning.
#[derive(Default)]
pub(super) struct Helpers {
    pub transforms: BTreeSet<&'static str>,
}

/// The transform-application expression, innermost first in declared order.
/// Only the language-specific `trim`/`lower`/`upper` spellings are Rust's;
/// the shared pipeline folds them and the case-fold helpers. Records a real
/// reference for every shared casing helper it reaches into `refs`, so the
/// import is collected and an SDK that never transforms casing ships none of
/// [`shared::casing_helpers`] at all (mirrors Go's own `apply_transforms`).
pub(super) fn apply_transforms(
    expr: String,
    transforms: &[String],
    helpers: &mut Helpers,
    refs: &mut Vec<Symbol>,
) -> String {
    // The fold's `helper` closure is `Fn`, not `FnMut` (the shared plan may
    // invoke it more than once), so the reached names are staged through a
    // `RefCell` and only pushed into `refs` after the call returns (mirrors
    // Go's own `apply_transforms`).
    let reached = std::cell::RefCell::new(Vec::new());
    let out = plan::apply_transforms(
        expr,
        transforms,
        &mut helpers.transforms,
        |t, out| match t {
            "trim" => Some(format!("({out}).trim().to_string()")),
            "lower" => Some(format!("({out}).to_lowercase()")),
            "upper" => Some(format!("({out}).to_uppercase()")),
            _ => None,
        },
        // The shared plan's own vocabulary is PascalCase (`StrUpperSnake`);
        // this crate's generated casing helper is snake_case like every
        // other Rust identifier here (`str_upper_snake`), so the fixed,
        // known vocabulary is translated directly rather than lowering only
        // the first letter (which would land on `strUpperSnake`, Go/
        // TypeScript's own spelling, not Rust's).
        |name| {
            let snake = match name {
                "StrUpperSnake" => "str_upper_snake",
                "StrSnake" => "str_snake",
                "StrKebab" => "str_kebab",
                "StrPascal" => "str_pascal",
                _ => name,
            };
            reached.borrow_mut().push(snake.to_string());
            shared_slot(snake)
        },
    );
    for name in reached.into_inner() {
        refs.push(shared_symbol(&name));
    }
    out
}

/// A reference to a name in one of the SDK's shared root groups, so the
/// import is collected wherever the raw text calls it.
pub(super) fn shared_symbol(name: &str) -> Symbol {
    Symbol::imported(name, shared_group(name), name)
}

/// A reference to a declaration of the SDK's shared support group (the
/// bespoke-facing transport types).
pub(super) fn support_symbol(name: &str) -> Symbol {
    Symbol::imported(name, crate::codegen::group::ROOT_SUPPORT, name)
}

/// A shared helper named inside opaque text: a slot, so its call spelling is
/// applied (or, for Rust, simply kept bare) when the file is rendered.
fn shared_slot(name: &str) -> String {
    crate::codegen::tree::symbol_slot(name)
}

/// Which SDK-root group declares a shared resolution helper, read off the
/// emitters rather than listed here so the answer cannot drift from where
/// the declaration actually lands.
fn shared_group(name: &str) -> String {
    shared::shared_groups()
        .into_iter()
        .find(|(_, decls)| {
            crate::codegen::imports::declared_symbols(decls)
                .iter()
                .any(|declared| declared == name)
        })
        .map(|(group, _)| crate::codegen::group::Group::root(group).path())
        .unwrap_or_default()
}

pub use plan::EntryEmission;

/// Emit a module's entries. The surface (Settings, options, the client
/// struct) and the behavior (the constructor, the operation methods) of one
/// entry are emitted together, so an entry's group holds the whole thing
/// rather than splitting it across a types and a codec file.
pub fn emit(module: &Module, config: &CasingConfig) -> EntryEmission {
    let Some((entries, multi, bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
        return EntryEmission::empty();
    };
    let mut shared = surface::config_structs(module, config);
    let decode_decls = plan::output_decode_decls(
        &entries,
        module,
        |op| wire_binding(op).is_some(),
        decode::output_decode_decl,
    );
    if !decode_decls.is_empty() {
        // `shared` rides the module's own types group ([`emit`]'s
        // `type_decls.extend(entries.shared)`), unlike a per-entry group
        // below, so a glob-import of that same group here would be a
        // self-import (`use crate::...::types::*;` inside types.rs itself,
        // a hard compile error): the decode helpers already see
        // TonoError/DecodeError/the output type without one.
        shared.extend(decode_decls);
    }
    let tested = crate::codegen::declared_tests::entries_with_tests(module);
    let mut per_entry = Vec::new();
    for entry in &entries {
        let n = names(entry, multi);
        // Scoped to this entry alone: each entry lands in its own group/file,
        // so its `use` imports must list only the resolution helpers its own
        // body calls, not the whole module's.
        let mut helpers = Helpers::default();
        let mut decls = vec![surface::settings_struct_decl(entry, &n, config, module)];
        decls.extend(constructor::construction_decls(
            entry,
            &n,
            module,
            config,
            &bound,
            &mut helpers,
            multi,
            tested.contains(entry.name),
        ));
        decls.extend(surface::discriminator_decls_for(entry, &n, module, &bound));
        // The whole surface freely names the module's error taxonomy and any
        // declared-error shape (`TonoError`, `ConfigError`, `APIFailure`, a
        // declared error's own type, ...) as bare text; a glob import of the
        // module's public group, the same technique the open enums' impls
        // already use, resolves them all without tracking a `Symbol` ref
        // through every place one of those names gets interpolated.
        decls.insert(
            0,
            crate::codegen::targets::rust::emit::types_glob_use(
                &crate::codegen::group::Group::types(&module.name),
            ),
        );
        per_entry.push((entry.name.to_string(), decls));
    }
    EntryEmission { shared, per_entry }
}

/// The rendered text of a module's whole entry emission (its shared group
/// followed by every entry's own group, concatenated), for a test that wants
/// to assert over the surface as a whole rather than one group at a time —
/// the shape the group split has no bearing on.
#[cfg(test)]
pub(super) fn entry_text(module: &Module, config: &CasingConfig) -> String {
    let emission = emit(module, config);
    let mut decls = emission.shared;
    for (_, entry_decls) in emission.per_entry {
        decls.extend(entry_decls);
    }
    crate::codegen::test_support::rendered(&decls, &RustRules::default())
}

/// The godoc-equivalent `@doc`/`@deprecated` block for an entry field's
/// public surface (a Settings field or a `.with_*` method), indented and
/// newline-terminated, or empty when the field carries neither trait. Shared
/// by `constructor` (a `.with_*` method's doc) and `surface` (a Settings
/// field's doc).
pub(super) fn field_doc(traits: &[Trait], indent_str: &str) -> String {
    let mut out = String::new();
    if let Some(d) = doc_of(traits) {
        out.push_str(&crate::codegen::doc::rustdoc(&d, indent_str));
    }
    if let Some(reason) = deprecated_of(traits) {
        out.push_str(&crate::codegen::targets::rust::render::deprecated_attr(
            Some(&reason),
        ));
        out.push('\n');
    }
    out
}

/// One `use crate::...` path for a bound bespoke symbol's file: `ext/rust/
/// sign.rs` becomes `ext::rust::sign` (the render engine prepends `crate::`
/// through the normal Symbol import machinery this returns a module path
/// for). Shared by `constructor` (the `client_init` hook) and `impl_op` (a
/// bespoke op's bound symbol).
pub(super) fn use_path(module: &str) -> String {
    module
        .strip_suffix(".rs")
        .unwrap_or(module)
        .replace('/', "::")
}

/// Indent every non-empty line of `s` by `n` four-space units.
pub(super) fn indent(s: &str, n: usize) -> String {
    let pad = "    ".repeat(n);
    s.trim_end_matches('\n')
        .split('\n')
        .map(|l| {
            if l.is_empty() {
                String::new()
            } else {
                format!("{pad}{l}\n")
            }
        })
        .collect()
}

/// One async (or sync, per `effect_of`) method per operation.
#[allow(clippy::too_many_arguments)]
fn op_methods(
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
    let ret = output.map(rust_type).unwrap_or_else(|| "()".into());

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
    let success = decode::success_block(output, module, "&outcome.body");
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
            before_request: hook_binding(bound, "before_request"),
            after_response: hook_binding(bound, "after_response"),
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

/// The per-type output-decode helpers every protocol operation needs
/// (`decode::success_block` is called only for the protocol path here — a
/// bespoke impl, typed or raw, handles its own output without it), deduped
/// by shape so operations sharing an output type share one `decode_<type>`
/// instead of each repeating its required-member probe.
mod checks;
#[cfg(test)]
mod conformance_tests;
mod constructor;
mod decode;
pub mod ext_fixtures;
mod impl_op;
mod resolve;
mod resolve_call;
mod resolve_env;
mod resolve_requires;
mod resolve_wire_call;
mod shared;
mod surface;
#[cfg(test)]
mod tests;
pub(crate) mod transport;
pub(crate) mod transport_decls;
pub(crate) mod vector_tests;

pub use shared::shared_groups;

//! The Go entry client: the SDK construction surface an entry declares,
//! spelled as idiomatic Go. `New` takes the `@arg` fields positionally and the
//! `@with` fields as functional options, resolves every declared source chain
//! top-down (explicit value wins over the chain, `@default` is the last
//! resort), parses env values by the field's type at the boundary (the error
//! names the variable and the type), lowers a match selection to a `switch`,
//! decodes a structured source strictly, composes a config through `@bind`,
//! and validates last. Each operation method reads the typed resolved
//! `Settings` directly and drives the SDK's own emitted transport (see
//! [`send`] and [`transport`]); no runtime package and no values bag exist.

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    deprecated_of, doc_of, field_ident, prim_spelling, rename_of, type_ident_from_id, wire_key,
};
use crate::codegen::entries::plan;
use crate::codegen::entries::{companion_name, op_local_name, ref_is_enum, EntryModel};
use crate::codegen::extensions::{impl_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_binding};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::go::types::{type_expr_of, GoVal, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{
    EntryField, EnvName, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Trait, Tref,
};

const BINDING_LANGS: [&str; 1] = ["go"];

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

/// A reference to a declaration of the SDK's shared (public) support group:
/// the canonical transport shapes live there, beside the branded well-known
/// types.
pub(super) fn support_symbol(name: &str) -> Symbol {
    Symbol::imported(name, crate::codegen::group::ROOT_SUPPORT, name)
}

pub(super) fn import(name: &str, module: &str) -> Symbol {
    Symbol::imported(name, module, name)
}

/// The per-entry generated names, derived once: unprefixed in a single-entry
/// module, entry-prefixed when several entries share the module.
struct Names {
    client: String,
    settings: String,
    option: String,
    carrier: String,
    new_fn: String,
    api: String,
    /// The canonical prefix for per-op companions (descriptor vars,
    /// discriminators): empty for a single entry.
    op_prefix: String,
}

fn names(entry: &EntryModel<'_>, multi: bool) -> Names {
    Names {
        client: pascal(entry.name),
        settings: pascal(&companion_name(entry.name, "settings", multi)),
        // The option type (and its private carrier) is always entry-prefixed:
        // a bare "Option" reads like a general-purpose type in godoc, and the
        // per-field With* names already carry the collision rule.
        option: pascal(&format!("{}_option", entry.name)),
        carrier: camel(&format!("{}_options", entry.name)),
        new_fn: if multi {
            format!("New{}", pascal(entry.name))
        } else {
            "New".to_string()
        },
        api: pascal(&format!("{}_api", entry.name)),
        op_prefix: if multi {
            format!("{}_", entry.name)
        } else {
            String::new()
        },
    }
}

/// The Go spelling of a type inside opaque text. An imported leaf renders as a
/// slot, so the package selector (or its absence) is decided when the file is
/// rendered; the caller declares the matching symbols with
/// [`push_type_symbols`].
pub(super) fn go_type(t: &Tref) -> String {
    render_type(&type_expr_of(t), &crate::codegen::targets::go::SlotRules)
}

/// The Go spelling of an operation's return position: a nullable (`T?`)
/// return becomes a pointer so it can be absent, except a collection, which
/// is already nullable (nil), mirroring `render_field`'s pointer rule.
pub(super) fn go_ret_type(t: &Tref, nullable: bool) -> String {
    let base = go_type(t);
    if nullable && !matches!(t, Tref::List(_) | Tref::Map(_, _)) {
        format!("*{base}")
    } else {
        base
    }
}

/// The Go spelling of a type as *data*: the name that goes into a message a
/// consumer reads, not into a code position. A slot would be wrong here twice
/// over: it never reaches the renderer intact through a quoted literal, and a
/// package selector is noise in an error string.
pub(super) fn go_type_label(t: &Tref) -> String {
    render_type(
        &type_expr_of(t),
        &crate::codegen::targets::go::GoRules::default(),
    )
}

/// The unexported Go name of a composed config type. A config is construction
/// only (never on the wire, never named by a caller), so it is hidden from the
/// package's public surface: an unexported type the SDK builds internally and
/// exposes only through the (already public) resolved values.
fn config_type_ident(id: &str) -> String {
    let name = type_ident_from_id(id);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_lowercase().chain(chars).collect(),
        None => name,
    }
}

/// The Go type spelling of an entry field, hiding a composed config behind its
/// unexported name, spelling a foreign opaque handle as the real package's
/// assumed exported type (pointer or interface value, see
/// [`ext::handle_go_type`]), while every other type keeps its normal (wire)
/// spelling.
fn field_go_type(t: &Tref, module: &Module, refs: &mut Vec<Symbol>) -> String {
    if let Tref::Ref { id, .. } = t {
        if module
            .shapes
            .iter()
            .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Config { .. }))
        {
            return config_type_ident(id);
        }
        if let Some((lib, type_name)) = ext::foreign_handle(t, module) {
            let handle = lib.types.iter().find(|ty| ty.name == type_name);
            if let Some(handle) = handle {
                if let Some(ty) = ext::handle_go_type(lib, handle, module, refs) {
                    return ty;
                }
            }
        }
    }
    go_type(t)
}

/// The Go type spelling of an entry field's own *storage* position (the
/// Settings struct field and the `@with` carrier field): identical to
/// [`field_go_type`] except a foreign opaque handle spells as tono's own
/// generated interface rather than the real package's own type,
/// so a hermetic declared test can fake it without the real library. Every
/// concrete value the field can hold (the real construction call's result, or
/// a `@with` setter's concrete-typed argument) still satisfies the interface
/// implicitly.
pub(super) fn field_go_type_storage(t: &Tref, module: &Module) -> String {
    if let Tref::Ref { id, .. } = t {
        if module
            .shapes
            .iter()
            .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Config { .. }))
        {
            return config_type_ident(id);
        }
        if let Some((lib, type_name)) = ext::foreign_handle(t, module) {
            if let Some(ty) = ext::handle_iface_type(lib, &type_name) {
                return ty;
            }
        }
    }
    go_type(t)
}

/// [`push_type_symbols`], but for an entry field's own declared type at a
/// *storage* position (Settings field, `@with` carrier, `@arg` param): a
/// foreign opaque handle spells as tono's own generated interface there
/// ([`field_go_type_storage`]), which is a local type, so nothing to
/// import. A position that spells the real concrete type instead (a `With*`
/// setter's own parameter) pulls the `ext` block's Go import itself, via
/// [`ext::handle_symbol`], rather than through this helper.
fn push_field_type_symbols(t: &Tref, module: &Module, refs: &mut Vec<Symbol>) {
    if ext::foreign_handle(t, module).is_some() {
        return;
    }
    push_type_symbols(t, refs);
}

/// The symbols a declared type references (for transitive import collection
/// off a raw declaration): the cross-module refs a nominal type carries.
fn push_type_symbols(t: &Tref, refs: &mut Vec<Symbol>) {
    fn walk(e: &crate::codegen::tree::TypeExpr, refs: &mut Vec<Symbol>) {
        use crate::codegen::tree::TypeExpr;
        match e {
            TypeExpr::Ref(s) => refs.push(s.clone()),
            TypeExpr::List(inner) | TypeExpr::Nullable(inner) => walk(inner, refs),
            TypeExpr::Map(k, v) | TypeExpr::Entries(k, v) => {
                walk(k, refs);
                walk(v, refs);
            }
            TypeExpr::Generic(s, args) => {
                refs.push(s.clone());
                for a in args {
                    walk(a, refs);
                }
            }
        }
    }
    walk(&type_expr_of(t), refs);
}

fn field_pascal(name: &str, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, None)
}

/// An entry field's exported identifier, honoring its `@rename(go)` override.
/// Config members and path tails keep the plain [`field_pascal`] (they are not
/// entry fields, so `@rename` on them is a later concern).
fn field_pascal_ren(name: &str, rename: Option<&str>, config: &CasingConfig) -> String {
    transform(name, SymbolKind::Field, config, rename)
}

/// An entry field's Go identifier: its exported `@rename(go)` spelling, or
/// an unexported camelCase name when the field's
/// declared type is a foreign opaque handle — a handle never reaches the
/// public surface, renamed or not.
pub(super) fn entry_field_ident(
    entry: &EntryModel<'_>,
    module: &Module,
    config: &CasingConfig,
    name: &str,
) -> String {
    if let Some(field) = entry.fields.iter().find(|f| f.name == name) {
        if ext::foreign_handle(&field.target, module).is_some() {
            return camel(name);
        }
    }
    field_pascal_ren(name, entry.field_rename(name, LANG).as_deref(), config)
}

/// The godoc `@doc` block plus a `// Deprecated:` line for an entry field's
/// public surface (a Settings/config struct field or a With* option), indented
/// and newline-terminated, or empty when the field carries neither trait.
fn field_doc(traits: &[Trait], indent: &str) -> String {
    let mut out = String::new();
    if let Some(d) = doc_of(traits) {
        out.push_str(&crate::codegen::doc::godoc(&d, indent));
    }
    let dep =
        crate::codegen::targets::go::render::deprecated_comment(deprecated_of(traits).as_deref());
    if !dep.is_empty() {
        out.push_str(&format!("{indent}{dep}\n"));
    }
    out
}

/// An expression casting a raw string `v` into the field's Go type. Only valid
/// for string-like types.
fn cast_string(t: &Tref, v: &str) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => v.to_string(),
        _ => format!("{}({v})", go_type(t)),
    }
}

/// An expression rendering a resolved field as a string for template
/// concatenation. [`as_string_needs_fmt`] says when the spelling pulls the
/// fmt import, which the call site owns.
fn as_string(expr: &str, t: &Tref) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        Tref::Prim(Prim::Timestamp | Prim::Date | Prim::Duration) | Tref::Ref { .. } => {
            format!("string({expr})")
        }
        _ => format!("fmt.Sprint({expr})"),
    }
}

fn as_string_needs_fmt(t: &Tref) -> bool {
    !matches!(
        t,
        Tref::Prim(Prim::String | Prim::Uuid | Prim::Timestamp | Prim::Date | Prim::Duration)
            | Tref::Ref { .. }
    )
}

/// The Go literal for a `@default`/match-arm JSON value of the given type.
fn literal(t: &Tref, v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => cast_string(t, &format!("{s:?}")),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => match t {
            Tref::Prim(Prim::Float) => format!("float64({n})"),
            Tref::Prim(_) => format!("{}({n})", go_type(t)),
            _ => n.to_string(),
        },
        other => format!("{other}"),
    }
}

/// A match-arm pattern as a Go `case` literal (an untyped constant converts to
/// the subject's named type on its own).
fn pattern_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}"),
        other => format!("{other}"),
    }
}

/// Which shared helper functions the emitted code pulled in; each is emitted
/// once per module.
#[derive(Default)]
struct Helpers {
    transforms: BTreeSet<&'static str>,
}

pub use plan::EntryEmission;

/// Emit a module's entries. The surface (Settings, options, the client struct)
/// and the behavior (the constructor, the operation methods) of one entry are
/// emitted together, so an entry's group holds the whole thing rather than
/// leaving the constructor in a file named for serialization.
///
/// The on-demand helpers are gathered across every entry and emitted once, since
/// Go would reject a second declaration of the same function in the package.
pub fn emit(module: &Module, config: &CasingConfig) -> EntryEmission {
    let Some((entries, multi, bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
        return EntryEmission::empty();
    };
    let tested: BTreeSet<String> = crate::codegen::declared_tests::entries_with_tests(module);
    let mut helpers = Helpers::default();
    let mut shared = surface::config_structs(module, config);
    shared.extend(handle_iface_decls(module, config));
    shared.extend(plan::output_decode_decls(
        &entries,
        module,
        |op| wire_binding(op).is_some() || impl_binding(&bound, &op.id).is_some_and(|b| b.raw),
        decode::output_decode_decl,
    ));
    let mut per_entry = Vec::new();
    for entry in &entries {
        let n = names(entry, multi);
        // The constructor comes right after the type.
        let mut decls = surface::entry_type_decls(entry, &n, module, config, multi);
        decls.extend(new_decl(
            entry,
            &n,
            module,
            config,
            &mut helpers,
            multi,
            tested.contains(entry.name),
        ));
        for op in entry.operations {
            decls.push(op_method_decl(entry, &n, op, module, config, &bound));
        }
        decls.extend(discriminator_decls_for(entry, &n, module, &bound));
        per_entry.push((entry.name.to_string(), decls));
    }
    EntryEmission { shared, per_entry }
}

/// The interface declaration of every foreign opaque handle the module's
/// `ext` block declares (deduplicated by lib+type, so two entries sharing one
/// handle never redeclare the same Go type in one package), emitted once as
/// shared machinery rather than per entry.
fn handle_iface_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let pairs: Vec<_> = module
        .ext_libs
        .iter()
        .flat_map(|lib| lib.types.iter().map(move |t| (lib, t)))
        .collect();
    let mut decls: Vec<Decl> = pairs
        .iter()
        .filter_map(|(lib, t)| ext::handle_iface_decl(lib, t))
        .collect();
    decls.extend(
        pairs
            .iter()
            .filter_map(|(lib, t)| ext::handle_adapter_decl(module, config, lib, t)),
    );
    decls
}

fn discriminator_name(n: &Names, op: &Shape) -> String {
    format!(
        "Decode{}Error",
        pascal(&format!("{}{}", n.op_prefix, op_local_name(&op.id)))
    )
}

/// The discrimination functions for the entry's operations (same shape as the
/// loose-op ones, named through the entry rule). An operation whose body is a
/// raw bespoke implementation gets the code-only variant under the same name:
/// its outcome carries no protocol status to match on.
fn discriminator_decls_for(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    bound: &[BoundExtension<'_>],
) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .filter_map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            let name = discriminator_name(n, op);
            if wire_binding(op).is_some() {
                return Some(super::errors::discriminator_fn_named(&name, &ordered));
            }
            // A typed impl already returns declared errors as typed values, so
            // it needs no discrimination at all.
            match impl_binding(bound, &op.id) {
                Some(b) if b.raw => Some(super::errors::outcome_discriminator_fn_named(
                    &name, &ordered,
                )),
                _ => None,
            }
        })
        .collect()
}

mod assembly;
#[cfg(test)]
mod bespoke_tests;
mod constructor;
mod decode;
mod ext;
/// IR builders shared by `ext_tests` (this crate's own unit tests) and the
/// `go_ext_roundtrip` integration test (a separate binary, which can only
/// see this crate's public API): not `cfg(test)`, so both can reuse the same
/// fixture builders instead of each declaring their own copy.
pub mod ext_fixtures;
#[cfg(test)]
mod ext_tests;
mod impl_op;
mod op_method;
mod resolve;
mod resolve_wire_call;
pub(crate) mod send;
mod shared;
mod surface;
#[cfg(test)]
mod tests;
mod transport;
pub(crate) mod vector_tests;
pub mod verify;

use constructor::{err_var, new_decl};
use op_method::{field_path_expr, op_method_decl, timeout_field_ident};
use resolve::Resolver;
use shared::{apply_transforms, shared_slot, shared_symbol};
pub use shared::{shared_groups, shared_groups_for};
use surface::method_signature;

/// Whether Go can coerce a logical value of `t` into `spelling` (see
/// `ext_render::coerce`); the reason names both types when it cannot.
pub fn param_spelling_coerces(
    module: &Module,
    lib: &crate::ir::ExtLib,
    t: &Tref,
    spelling: &str,
) -> Result<(), String> {
    ext::coerce(module, lib, t, spelling, "v", None).map(|_| ())
}

/// Whether Go can pass a literal of the foreign form `form` under
/// `spelling` (see `ext_render::form_coerce`); the reason names both types
/// when it cannot. A form with no `go` block is refused on its own.
pub fn form_spelling_coerces(
    module: &Module,
    lib: &crate::ir::ExtLib,
    form: &crate::ir::ForeignStruct,
    spelling: &str,
) -> Result<(), String> {
    match form.lang("go") {
        Some(block) => ext::form_coerce(module, lib, block, spelling, "v").map(|_| ()),
        None => Ok(()),
    }
}

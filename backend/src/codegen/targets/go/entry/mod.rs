//! The Go entry client: the SDK construction surface an entry declares,
//! spelled as idiomatic Go. `New` takes the `@arg` fields positionally and the
//! `@with` fields as functional options, resolves every declared source chain
//! top-down (explicit value wins over the chain, `@default` is the last
//! resort), parses env values by the field's type at the boundary (the error
//! names the variable and the type), lowers a match selection to a `switch`,
//! decodes a structured source strictly, composes a config through `@bind`,
//! runs the bound `client_init` hook over the resolved `Settings` (bespoke
//! wins), and validates last. Each operation method reads the typed resolved
//! `Settings` directly and drives the SDK's own emitted transport (see
//! [`send`] and [`transport`]); no runtime package and no values bag exist.

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    deprecated_of, doc_of, field_ident, prim_spelling, rename_of, type_ident_from_id, wire_key,
};
use crate::codegen::entries::plan;
use crate::codegen::entries::{companion_name, op_local_name, ref_is_enum, EntryModel};
use crate::codegen::extensions::{hook_binding, impl_binding, BoundExtension};
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
/// the transport shapes a bespoke hook types against live there, beside the
/// branded well-known types.
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
/// unexported name, spelling a foreign opaque handle as a pointer
/// to the real package's assumed exported type, while every other type keeps
/// its normal (wire) spelling.
fn field_go_type(t: &Tref, module: &Module) -> String {
    if let Tref::Ref { id, .. } = t {
        if module
            .shapes
            .iter()
            .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Config { .. }))
        {
            return config_type_ident(id);
        }
        if let Some((lib, type_name)) = ext::foreign_handle(t, module) {
            if let Some(ty) = ext::handle_go_type(lib, &type_name) {
                return ty;
            }
        }
    }
    go_type(t)
}

/// [`push_type_symbols`], but for an entry field's own declared type: a
/// foreign opaque handle pulls its `ext` block's Go import
/// instead of trying to resolve as an in-module reference.
fn push_field_type_symbols(t: &Tref, module: &Module, refs: &mut Vec<Symbol>) {
    if let Some((lib, _)) = ext::foreign_handle(t, module) {
        if let Some(sym) = ext::handle_symbol(lib) {
            refs.push(sym);
        }
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
    shared.extend(hook_wrapper_decls(&bound, &entries, multi));
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
        let mut decls = surface::entry_type_decls(entry, &n, module, config, multi, &bound);
        decls.extend(new_decl(
            entry,
            &n,
            module,
            config,
            &bound,
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

/// The bound-hook boundary wrappers. `client_init` receives the entry's
/// `Settings` (single-entry modules only: the bespoke symbol has one
/// signature); `before_request`/`after_response` carry the runtime shapes;
/// `on_error` maps the outgoing error.
fn hook_wrapper_decls(
    bound: &[BoundExtension<'_>],
    entries: &[EntryModel<'_>],
    multi: bool,
) -> Vec<Decl> {
    let en = error_names();
    let mut decls = Vec::new();
    let wrap = |slot: &str, call: &str, sig: &str, ret: &str| {
        format!(
            "func {name}{sig} {{\n\
             \t{ret}err := {call}\n\
             \tif err != nil {{\n\
             \t\tvar known {marker}\n\
             \t\tif errors.As(err, &known) {{\n\t\t\treturn {out}err\n\t\t}}\n\
             \t\treturn {out}&{contract}{{ContractName: \"{slot}\", Cause: err}}\n\
             \t}}\n\
             \treturn {out}nil\n\
             }}",
            name = hook_wrapper_name(slot),
            contract = en.contract,
            marker = super::errors::SDK_ERROR_MARKER,
            out = if ret.is_empty() { "" } else { "out, " },
            ret = if ret.is_empty() { "" } else { "out, " },
        )
    };
    if let Some(b) = hook_binding(bound, "client_init") {
        if !multi {
            if let Some(entry) = entries.first() {
                let n = names(entry, multi);
                decls.push(Decl::raw_with(
                    format!(
                        "// {name} is the boundary wrapper for the client_init hook: bespoke\n\
                         // code runs over the resolved Settings (bespoke wins) before validation.\n\
                         // The bespoke {sym} lives in this package (drop {module} into it).\n\
                         {body}",
                        name = hook_wrapper_name("client_init"),
                        sym = b.symbol,
                        module = b.module,
                        body = wrap(
                            "client_init",
                            &format!("{}(s)", b.symbol),
                            &format!("(s *{}) error", n.settings),
                            ""
                        ),
                    ),
                    vec![import("errors", "errors")],
                ));
            }
        }
    }
    for (slot, var, shape) in [
        ("before_request", "req", "HTTPRequest"),
        ("after_response", "res", "HTTPResponse"),
    ] {
        if let Some(b) = hook_binding(bound, slot) {
            let shape_slot = shared_slot(shape);
            decls.push(Decl::raw_with(
                wrap(
                    slot,
                    &format!("{}(ctx, {var})", b.symbol),
                    &format!("(ctx context.Context, {var} {shape_slot}) ({shape_slot}, error)"),
                    "out",
                ),
                vec![
                    import("errors", "errors"),
                    import("context", "context"),
                    support_symbol(shape),
                ],
            ));
        }
    }
    if let Some(b) = hook_binding(bound, "on_error") {
        decls.push(Decl::raw(format!(
            "// {name} maps every error leaving the SDK through the bound on_error hook.\n\
             func {name}(err error) error {{\n\treturn {sym}(err)\n}}",
            name = hook_wrapper_name("on_error"),
            sym = b.symbol,
        )));
    }
    decls
}

fn hook_wrapper_name(slot: &str) -> String {
    camel(&format!("{slot}_hook"))
}

/// The zero-value declaration and the return prefix a method needs to bail out
/// early. `var zero T` is the one zero spelling valid for every Go type (a
/// composite literal is not, for primitives).
pub(super) fn zero_of(output: Option<&Tref>) -> (String, &'static str) {
    match output {
        Some(t) => (format!("\tvar zero {}\n", go_type(t)), "zero, "),
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
fn op_method_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    bound: &[BoundExtension<'_>],
) -> Decl {
    let en = error_names();
    let (sig, mut refs) = method_signature(op, config);
    let has_on_error = hook_binding(bound, "on_error").is_some();
    let (input, output) = crate::codegen::ops::op_io(op);
    let fail = |expr: String| {
        if has_on_error {
            format!("{}({expr})", hook_wrapper_name("on_error"))
        } else {
            expr
        }
    };
    let (zero_decl, ret_zero) = zero_of(output);
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
    let module_hooks = hook_binding(bound, "before_request").is_some()
        || hook_binding(bound, "after_response").is_some();
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
        module_hooks,
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
mod resolve;
pub(crate) mod send;
mod shared;
mod surface;
#[cfg(test)]
mod tests;
mod transport;
pub(crate) mod vector_tests;

use constructor::{err_var, new_decl};
use resolve::Resolver;
use shared::{apply_transforms, shared_slot, shared_symbol};
pub use shared::{shared_groups, shared_groups_for};
use surface::method_signature;

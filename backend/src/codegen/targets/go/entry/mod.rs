//! The Go entry client: the SDK construction surface an entry declares,
//! spelled as idiomatic Go. `New` takes the `@arg` fields positionally and the
//! `@with` fields as functional options, resolves every declared source chain
//! top-down (explicit value wins over the chain, `@default` is the last
//! resort), parses env values by the field's type at the boundary (the error
//! names the variable and the type), lowers a match selection to a `switch`,
//! decodes a structured source strictly, composes a config through `@bind`,
//! runs the bound `client_init` hook over the resolved `Settings` (bespoke
//! wins), and validates last. The resolved fields are handed to the runtime as
//! `Options.Values`, so the descriptor's ref positions (endpoint, headers,
//! timeout, retry) resolve without the client ever interpreting a descriptor.

use std::collections::BTreeSet;

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::{
    deprecated_of, doc_of, prim_spelling, rename_of, type_ident_from_id, wire_key,
};
use crate::codegen::entries::{
    companion_name, module_entries, op_local_name, ref_is_enum, EntryModel,
};
use crate::codegen::extensions::{bound_extensions, hook_binding, impl_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_descriptor};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::go::render::GoRules;
use crate::codegen::targets::go::types::{type_expr_of, GoVal, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{
    EntryField, EnvName, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Trait, Tref,
};

/// The Go module path of the hand-written HTTP runtime the generated client
/// drives. The import spells the path; Go resolves the package name
/// (`tonohttp`) from the package clause.
pub const RUNTIME_MODULE: &str = "github.com/tono-lang/tono/runtimes/http-go";

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

fn runtime_symbol() -> Symbol {
    Symbol::imported("tonohttp", RUNTIME_MODULE, "tonohttp")
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

pub(super) fn go_type(t: &Tref) -> String {
    render_type(&type_expr_of(t), &GoRules::default())
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
/// unexported name while every other type keeps its normal (wire) spelling.
fn field_go_type(t: &Tref, module: &Module) -> String {
    if let Tref::Ref { id, .. } = t {
        if module
            .shapes
            .iter()
            .any(|s| s.id == *id && matches!(s.kind, ShapeKind::Config { .. }))
        {
            return config_type_ident(id);
        }
    }
    go_type(t)
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
    duration_ms: bool,
    transforms: BTreeSet<&'static str>,
}

/// Every serde-file declaration: the shared helpers, the bound-hook wrappers,
/// and per entry the descriptor constants, the constructor, and the methods.
pub fn serde_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let entries = module_entries(module);
    if entries.is_empty() {
        return Vec::new();
    }
    let multi = entries.len() > 1;
    let bound = bound_extensions(module, &BINDING_LANGS);
    let mut helpers = Helpers::default();
    let mut decls = vec![shared_helpers_decl()];
    decls.extend(hook_wrapper_decls(&bound, &entries, multi));
    for entry in &entries {
        let n = names(entry, multi);
        decls.extend(descriptor_decls(entry, &n));
        decls.push(new_decl(
            entry,
            &n,
            module,
            config,
            &bound,
            &mut helpers,
            multi,
        ));
        for op in entry.operations {
            decls.push(op_method_decl(&n, op, module, config, &bound));
        }
        decls.extend(discriminator_decls_for(entry, &n, module, &bound));
    }
    decls.extend(helper_decls(&helpers));
    decls
}

/// The always-needed helpers: descriptor parsing at package load and the
/// struct-to-record encoding the runtime input takes.
fn shared_helpers_decl() -> Decl {
    Decl::raw_with(
        "// mustDescriptor parses a compiler-emitted descriptor literal at package\n\
         // load; a parse failure is a build defect, never a runtime input.\n\
         func mustDescriptor(literal string) *tonohttp.WireDescriptor {\n\
         \td, err := tonohttp.ParseDescriptor([]byte(literal))\n\
         \tif err != nil {\n\t\tpanic(err)\n\t}\n\
         \treturn d\n\
         }\n\n\
         // encodeRecord turns a typed input into the wire record the runtime binds\n\
         // from, through the type's own JSON tags.\n\
         func encodeRecord(v any) (map[string]any, error) {\n\
         \tb, err := json.Marshal(v)\n\
         \tif err != nil {\n\t\treturn nil, err\n\t}\n\
         \tvar m map[string]any\n\
         \tif err := json.Unmarshal(b, &m); err != nil {\n\t\treturn nil, err\n\t}\n\
         \treturn m, nil\n\
         }"
        .to_string(),
        vec![runtime_symbol(), import("json", "encoding/json")],
    )
}

/// The on-demand helpers the resolution used.
fn helper_decls(helpers: &Helpers) -> Vec<Decl> {
    let mut decls = Vec::new();
    if helpers.duration_ms {
        decls.push(Decl::raw_with(
            "// durationMs parses a duration field for the runtime's millisecond value\n\
             // positions.\n\
             func durationMs(v string) (float64, error) {\n\
             \td, err := time.ParseDuration(v)\n\
             \tif err != nil {\n\t\treturn 0, err\n\t}\n\
             \treturn float64(d) / float64(time.Millisecond), nil\n\
             }"
            .to_string(),
            vec![import("time", "time")],
        ));
    }
    if !helpers.transforms.is_empty() {
        decls.push(Decl::raw_with(
            "// strTransformWords splits a resolved value for the casing transforms:\n\
             // runs of spaces, hyphens, and underscores separate words.\n\
             func strTransformWords(s string) []string {\n\
             \treturn strings.FieldsFunc(s, func(r rune) bool { return r == ' ' || r == '-' || r == '_' })\n\
             }"
            .to_string(),
            vec![import("strings", "strings")],
        ));
        for t in &helpers.transforms {
            let (name, body) = match *t {
                "upper_snake" => ("strUpperSnake", "\tws := strTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToUpper(ws[i])\n\t}\n\treturn strings.Join(ws, \"_\")"),
                "snake" => ("strSnake", "\tws := strTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToLower(ws[i])\n\t}\n\treturn strings.Join(ws, \"_\")"),
                "kebab" => ("strKebab", "\tws := strTransformWords(s)\n\tfor i := range ws {\n\t\tws[i] = strings.ToLower(ws[i])\n\t}\n\treturn strings.Join(ws, \"-\")"),
                "pascal" => ("strPascal", "\tws := strTransformWords(s)\n\tfor i := range ws {\n\t\tif ws[i] != \"\" {\n\t\t\tws[i] = strings.ToUpper(ws[i][:1]) + strings.ToLower(ws[i][1:])\n\t\t}\n\t}\n\treturn strings.Join(ws, \"\")"),
                _ => continue,
            };
            decls.push(Decl::raw_with(
                format!("func {name}(s string) string {{\n{body}\n}}"),
                vec![import("strings", "strings")],
            ));
        }
    }
    decls
}

/// The transform-application expression, innermost first in declared order.
/// Only the language-specific `trim`/`lower`/`upper` spellings are Go's; the
/// shared pipeline folds them and the case-fold helpers.
fn apply_transforms(expr: String, transforms: &[String], helpers: &mut Helpers) -> String {
    crate::codegen::entries::plan::apply_transforms(
        expr,
        transforms,
        &mut helpers.transforms,
        |t, out| match t {
            "trim" => Some(format!("strings.TrimSpace({out})")),
            "lower" => Some(format!("strings.ToLower({out})")),
            "upper" => Some(format!("strings.ToUpper({out})")),
            _ => None,
        },
    )
}

/// One `var <op>Descriptor = mustDescriptor(...)` per operation. The literal
/// is the descriptor serialized to a Go string: an opaque blob, never read.
fn descriptor_decls(entry: &EntryModel<'_>, n: &Names) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter_map(|op| {
            let descriptor = wire_descriptor(op)?;
            let json = serde_json::to_string(descriptor).unwrap_or_else(|_| "null".into());
            Some(Decl::raw(format!(
                "var {var} = mustDescriptor({literal})",
                var = descriptor_var(n, op),
                literal = go_string_literal(&json),
            )))
        })
        .collect()
}

fn descriptor_var(n: &Names, op: &Shape) -> String {
    camel(&format!(
        "{}{}_descriptor",
        n.op_prefix,
        op_local_name(&op.id)
    ))
}

fn discriminator_name(n: &Names, op: &Shape) -> String {
    format!(
        "Decode{}Error",
        pascal(&format!("{}{}", n.op_prefix, op_local_name(&op.id)))
    )
}

/// The descriptor text as a Go interpreted string literal. JSON string
/// escaping is a subset of Go's escape sequences (\", \\, \uXXXX), so
/// serializing the text as a JSON string yields a valid Go literal with every
/// control character escaped, the same double-encoding the TypeScript embed
/// uses.
fn go_string_literal(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
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
            if wire_descriptor(op).is_some() {
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
        ("before_request", "req", "CanonicalRequest"),
        ("after_response", "res", "CanonicalResponse"),
    ] {
        if let Some(b) = hook_binding(bound, slot) {
            decls.push(Decl::raw_with(
                wrap(
                    slot,
                    &format!("{}(ctx, {var})", b.symbol),
                    &format!(
                        "(ctx context.Context, {var} tonohttp.{shape}) (tonohttp.{shape}, error)"
                    ),
                    "out",
                ),
                vec![
                    import("errors", "errors"),
                    import("context", "context"),
                    runtime_symbol(),
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

/// One concrete client method: encode the input record, hand the descriptor to
/// the runtime, and map the raw outcome onto the generated taxonomy.
fn op_method_decl(
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
    let validate_block = validate_block(input, module, ret_zero, &fail);
    if wire_descriptor(op).is_none() {
        // No protocol binding: the operation is implemented by bespoke sources
        // the frontend proved are bound, and the generator gate proved are bound
        // for this target.
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
    }
    refs.push(runtime_symbol());
    let record = match input {
        Some(_) => format!(
            "\trecord, err := encodeRecord(input)\n\
             \tif err != nil {{\n\t\treturn {ret_zero}{fail_enc}\n\t}}\n",
            fail_enc = fail("err".to_string()),
        ),
        None => "\tvar record map[string]any\n".to_string(),
    };
    let error_expr = if declared_errors(op, module).is_empty() {
        format!(
            "&{api}{{Status: outcome.Status, Body: outcome.Body}}",
            api = en.api
        )
    } else {
        format!(
            "{}(outcome.Status, []byte(outcome.Body))",
            discriminator_name(n, op)
        )
    };
    let success = decode::success_block(
        output,
        module,
        &decode::Payload {
            text: "outcome.Body",
            bytes: "[]byte(outcome.Body)",
        },
        &fail,
        &mut refs,
    );
    let doc = doc_of(&op.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let text = format!(
        "{doc}func (c *{client}) {sig} {{\n\
         {zero_decl}{validate_block}{record}\
         \toutcome, err := c.runtime.Execute(ctx, {descriptor}, record, c.hooks)\n\
         \tif err != nil {{\n\t\treturn {ret_zero}{fail_hook}\n\t}}\n\
         \tswitch outcome.Kind {{\n\
         \tcase tonohttp.OutcomeTransport:\n\t\treturn {ret_zero}{fail_transport}\n\
         \tcase tonohttp.OutcomeError:\n\t\treturn {ret_zero}{fail_api}\n\
         \t}}\n\
         {success}\n\
         }}",
        client = n.client,
        descriptor = descriptor_var(n, op),
        fail_hook = fail("err".to_string()),
        fail_transport = fail(format!(
            "&{transport}{{Cause: outcome.Cause}}",
            transport = en.transport
        )),
        fail_api = fail(error_expr),
    );
    Decl::raw_with(text, refs)
}

mod constructor;
mod decode;
mod impl_op;
mod resolve;
mod surface;
#[cfg(test)]
mod tests;

use constructor::{new_decl, why_var};
use resolve::Resolver;
use surface::method_signature;
pub use surface::type_decls;

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
use crate::codegen::conventions::{doc_of, prim_spelling, rename_of, type_ident_from_id};
use crate::codegen::entries::{
    companion_name, module_entries, op_local_name, EntryModel, FieldShape,
};
use crate::codegen::extensions::{bound_extensions, hook_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, wire_descriptor};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::go::render::GoRules;
use crate::codegen::targets::go::types::{type_expr_of, GoVal, LANG};
use crate::codegen::tree::Decl;
use crate::codegen::validation;
use crate::ir::{
    ArmValue, EntryField, EnvName, Member, Module, Prim, Shape, ShapeKind, Source, TemplatePart,
    Tref,
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

fn import(name: &str, module: &str) -> Symbol {
    Symbol::imported(name, module, name)
}

/// The per-entry generated names, derived once: unprefixed in a single-entry
/// module, entry-prefixed when several entries share the module.
struct Names {
    client: String,
    settings: String,
    option: String,
    withs: String,
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
        withs: camel(&format!("{}_withs", entry.name)),
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

fn go_type(t: &Tref) -> String {
    render_type(&type_expr_of(t), &GoRules::default())
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

/// Whether the resolved value is string-shaped in Go (assignable from a raw
/// env string with at most a named-type cast).
fn string_like(t: &Tref) -> bool {
    matches!(
        t,
        Tref::Prim(Prim::String | Prim::Uuid | Prim::Timestamp | Prim::Date | Prim::Duration)
            | Tref::Ref { .. }
    )
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
/// concatenation.
fn as_string(expr: &str, t: &Tref, helpers: &mut Helpers) -> String {
    match t {
        Tref::Prim(Prim::String | Prim::Uuid) => expr.to_string(),
        Tref::Prim(Prim::Timestamp | Prim::Date | Prim::Duration) | Tref::Ref { .. } => {
            format!("string({expr})")
        }
        _ => {
            helpers.fmt = true;
            format!("fmt.Sprint({expr})")
        }
    }
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
    fmt: bool,
    duration_ms: bool,
    transforms: BTreeSet<&'static str>,
}

/// The construction-only config structs. They never cross the wire, so the
/// regular type emission skips them; the entry surface is what makes them
/// real (as plain structs the resolved `Settings` embeds).
fn config_structs(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    module
        .shapes
        .iter()
        .filter_map(|shape| {
            let ShapeKind::Config { fields } = &shape.kind else {
                return None;
            };
            let name = type_ident_from_id(&shape.id);
            let members: String = fields
                .iter()
                .map(|f| {
                    format!(
                        "\t{} {}\n",
                        field_pascal(&f.name, config),
                        go_type(&f.target)
                    )
                })
                .collect();
            Some(Decl::raw(format!(
                "// {name} is a construction-only composition of the entry surface; it\n\
                 // never crosses the wire.\n\
                 type {name} struct {{\n{members}}}"
            )))
        })
        .collect()
}

/// The `Settings` struct: every resolved entry field plus the transport slots
/// the bespoke `client_init` hook may fill (native client or canonical
/// transport, and the base headers a bespoke auth writes into).
fn settings_decl(entry: &EntryModel<'_>, n: &Names, config: &CasingConfig) -> Decl {
    let mut fields = String::new();
    for f in entry.declared() {
        fields.push_str(&format!(
            "\t{} {}\n",
            field_pascal(&f.name, config),
            go_type(&f.target)
        ));
    }
    let text = format!(
        "// {settings} are the resolved construction values of the {entry} entry,\n\
         // handed to the client_init hook before validation: bespoke code may\n\
         // overwrite any field (bespoke wins) and set transport through the slots.\n\
         // Exactly one transport slot may be set: HTTPClient (native) or Transport\n\
         // (canonical). Headers are the base request headers (bespoke auth writes\n\
         // here); a declared @header wins only where nothing else set the name.\n\
         type {settings} struct {{\n{fields}\n\tHTTPClient *http.Client\n\tTransport  tonohttp.Transport\n\tHeaders    map[string]string\n}}",
        settings = n.settings,
        entry = entry.name,
    );
    Decl::raw_with(text, vec![import("http", "net/http"), runtime_symbol()])
}

/// The functional-option surface: one `With*` per `@with` field over a private
/// carrier struct (a pointer per field, so an unset option is distinguishable
/// from a zero value).
fn option_decls(entry: &EntryModel<'_>, n: &Names, multi: bool) -> Vec<Decl> {
    let withs = entry.withs();
    if withs.is_empty() {
        return Vec::new();
    }
    let mut decls = Vec::new();
    let carrier_fields: String = withs
        .iter()
        .map(|f| format!("\t{} *{}\n", camel(&f.name), go_type(&f.target)))
        .collect();
    decls.push(Decl::raw(format!(
        "// {option} configures an optional (@with) construction value of {client}.\n\
         type {option} func(*{withs})\n\n\
         type {withs} struct {{\n{carrier_fields}}}",
        option = n.option,
        client = n.client,
        withs = n.withs,
    )));
    for f in withs {
        let fn_name = pascal(&format!(
            "with_{}",
            companion_name(entry.name, &f.name, multi)
        ));
        decls.push(Decl::raw(format!(
            "// {fn_name} sets the {field} construction value.\n\
             func {fn_name}(v {ty}) {option} {{\n\treturn func(w *{withs}) {{ w.{carrier} = &v }}\n}}",
            field = f.name,
            ty = go_type(&f.target),
            option = n.option,
            withs = n.withs,
            carrier = camel(&f.name),
        )));
    }
    decls
}

/// The client struct, its mock interface (one method per operation, `ctx`
/// first), and the compile-time conformance assertion.
fn client_decls(entry: &EntryModel<'_>, n: &Names, config: &CasingConfig) -> Vec<Decl> {
    let mut methods = String::new();
    let mut refs = vec![import("context", "context")];
    for op in entry.operations {
        let (sig, sig_refs) = method_signature(op, config);
        refs.extend(sig_refs);
        methods.push_str(&format!("\t{sig}\n"));
    }
    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    vec![
        Decl::raw_with(
            format!(
                "{doc}// {client} is the generated SDK client the {entry} entry declares.\n\
                 type {client} struct {{\n\tsettings {settings}\n\truntime  *tonohttp.Runtime\n\thooks    *tonohttp.Hooks\n}}",
                client = n.client,
                entry = entry.name,
                settings = n.settings,
            ),
            vec![runtime_symbol()],
        ),
        Decl::raw_with(
            format!(
                "// {api} is the operation surface of {client}, for mocking.\n\
                 type {api} interface {{\n{methods}}}\n\n\
                 var _ {api} = (*{client})(nil)",
                api = n.api,
                client = n.client,
            ),
            refs,
        ),
    ]
}

/// One operation's Go method signature (shared by the interface and the
/// concrete method): `Name(ctx context.Context, input In) (Out, error)`.
fn method_signature(op: &Shape, config: &CasingConfig) -> (String, Vec<Symbol>) {
    let name = method_name(op, config);
    let (input, output) = crate::codegen::ops::op_io(op);
    let mut refs = vec![import("context", "context")];
    let param = match input {
        Some(t) => {
            push_type_symbols(t, &mut refs);
            format!(", input {}", go_type(t))
        }
        None => String::new(),
    };
    let ret = match output {
        Some(t) => {
            push_type_symbols(t, &mut refs);
            format!("({}, error)", go_type(t))
        }
        None => "error".to_string(),
    };
    (format!("{name}(ctx context.Context{param}) {ret}"), refs)
}

fn method_name(op: &Shape, config: &CasingConfig) -> String {
    let rename = rename_of(&op.traits, LANG);
    transform(
        op_local_name(&op.id),
        SymbolKind::Method,
        config,
        rename.as_deref(),
    )
}

/// Every type-file declaration of the module's entry surface.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let entries = module_entries(module);
    if entries.is_empty() {
        return Vec::new();
    }
    let multi = entries.len() > 1;
    let mut decls = config_structs(module, config);
    for entry in &entries {
        let n = names(entry, multi);
        decls.push(settings_decl(entry, &n, config));
        decls.extend(option_decls(entry, &n, multi));
        decls.extend(client_decls(entry, &n, config));
    }
    decls
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
        decls.extend(discriminator_decls_for(entry, &n, module));
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
fn apply_transforms(expr: String, transforms: &[String], helpers: &mut Helpers) -> String {
    let mut out = expr;
    for t in transforms {
        out = match t.as_str() {
            "trim" => format!("strings.TrimSpace({out})"),
            "lower" => format!("strings.ToLower({out})"),
            "upper" => format!("strings.ToUpper({out})"),
            "upper_snake" | "snake" | "kebab" | "pascal" => {
                helpers.transforms.insert(match t.as_str() {
                    "upper_snake" => "upper_snake",
                    "snake" => "snake",
                    "kebab" => "kebab",
                    _ => "pascal",
                });
                format!("str{}({out})", pascal(t))
            }
            _ => out,
        };
    }
    out
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

/// A Go interpreted string literal for arbitrary text.
fn go_string_literal(s: &str) -> String {
    format!("{s:?}")
}

/// The discrimination functions for the entry's operations (same shape as the
/// loose-op ones, named through the entry rule).
fn discriminator_decls_for(entry: &EntryModel<'_>, n: &Names, module: &Module) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            super::errors::discriminator_fn_named(&discriminator_name(n, op), &ordered)
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
             \t\tvar wrapped *{contract}\n\
             \t\tif errors.As(err, &wrapped) {{\n\t\t\treturn {out}err\n\t\t}}\n\
             \t\treturn {out}&{contract}{{ContractName: \"{slot}\", Cause: err}}\n\
             \t}}\n\
             \treturn {out}nil\n\
             }}",
            name = hook_wrapper_name(slot),
            contract = en.contract,
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
    if let Some(b) = hook_binding(bound, "before_request") {
        decls.push(Decl::raw_with(
            wrap(
                "before_request",
                &format!("{}(ctx, req)", b.symbol),
                "(ctx context.Context, req tonohttp.CanonicalRequest) (tonohttp.CanonicalRequest, error)",
                "out",
            ),
            vec![
                import("errors", "errors"),
                import("context", "context"),
                runtime_symbol(),
            ],
        ));
    }
    if let Some(b) = hook_binding(bound, "after_response") {
        decls.push(Decl::raw_with(
            wrap(
                "after_response",
                &format!("{}(ctx, res)", b.symbol),
                "(ctx context.Context, res tonohttp.CanonicalResponse) (tonohttp.CanonicalResponse, error)",
                "out",
            ),
            vec![
                import("errors", "errors"),
                import("context", "context"),
                runtime_symbol(),
            ],
        ));
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
    if wire_descriptor(op).is_none() {
        // An operation without a transport binding is bespoke-bound; invoking
        // the bound impl through the generated glue is not wired yet, so the
        // method reports that plainly instead of failing on a missing
        // descriptor.
        let (_, output) = crate::codegen::ops::op_io(op);
        let zero = match output {
            Some(t) => format!("{}{{}}, ", go_type(t)),
            None => String::new(),
        };
        refs.push(import("errors", "errors"));
        return Decl::raw_with(
            format!(
                "func (c *{client}) {sig} {{\n\treturn {zero}&{contract}{{ContractName: {op:?}, Cause: errors.New(\"operation has no transport binding\")}}\n}}",
                client = n.client,
                contract = en.contract,
                op = op_local_name(&op.id),
            ),
            refs,
        );
    }
    refs.push(runtime_symbol());
    let (input, output) = crate::codegen::ops::op_io(op);
    let has_on_error = hook_binding(bound, "on_error").is_some();
    let fail = |expr: String| {
        if has_on_error {
            format!("{}({expr})", hook_wrapper_name("on_error"))
        } else {
            expr
        }
    };
    let (zero_decl, ret_zero) = match output {
        Some(t) => (format!("\tvar zero {}\n", go_type(t)), "zero, "),
        None => (String::new(), ""),
    };
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
    let success = match output {
        Some(t) => {
            refs.push(import("json", "encoding/json"));
            format!(
                "\tvar out {ty}\n\
                 \tif err := json.Unmarshal([]byte(outcome.Body), &out); err != nil {{\n\
                 \t\treturn zero, {fail_decode}\n\t}}\n\
                 \treturn out, nil",
                ty = go_type(t),
                fail_decode = fail(format!(
                    "&{decode}{{Path: \"$\", Expected: {expected:?}, Raw: outcome.Body}}",
                    decode = en.decode,
                    expected = go_type(t),
                )),
            )
        }
        None => "\treturn nil".to_string(),
    };
    let doc = doc_of(&op.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    let text = format!(
        "{doc}func (c *{client}) {sig} {{\n\
         {zero_decl}{record}\
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

/// The generated constructor. The body follows the declared order exactly:
/// sources resolve top-down, `client_init` runs over the result (bespoke
/// wins), the consumed chains and declared constraints validate last, and the
/// resolved values are frozen into the runtime options.
#[allow(clippy::too_many_arguments)]
fn new_decl(
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
    let opts_param = if entry.withs().is_empty() {
        String::new()
    } else {
        format!(
            "{}opts ...{}",
            if params.is_empty() { "" } else { ", " },
            n.option
        )
    };

    if !entry.withs().is_empty() {
        body.push_str(&format!(
            "\tw := {withs}{{}}\n\tfor _, opt := range opts {{\n\t\topt(&w)\n\t}}\n",
            withs = n.withs
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
    // call obscurely.
    let heads = entry.consumed_field_heads();
    for head in &heads {
        let Some(field) = entry.fields.iter().find(|f| f.name == *head) else {
            continue;
        };
        if entry.is_guaranteed(field) || !string_like(&field.target) {
            continue;
        }
        if matches!(
            entry.field_shape(field, module),
            FieldShape::Config(_) | FieldShape::Structured(_) | FieldShape::Json
        ) {
            continue;
        }
        refs.push(import("errors", "errors"));
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
            let guard = if entry.is_guaranteed(field) || composed {
                line.condition.clone()
            } else {
                format!("{} == \"\" && {}", why_var(&field.name), line.condition)
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
        let Some(expr) = value_expr(&vp, config) else {
            continue;
        };
        // A composed config is always constructed, so its members are always
        // readable (an unresolved member is its zero value); only a scalar
        // chain and a structured decode track absence.
        let composed = matches!(entry.field_shape(vp.field, module), FieldShape::Config(_));
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
        match presence_guard(entry, &vp).filter(|_| !composed) {
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

fn indent(block: &str) -> String {
    block
        .lines()
        .map(|l| format!("\t{l}\n"))
        .collect::<String>()
}

fn why_var(field: &str) -> String {
    camel(&format!("{field}_why"))
}

/// The Go expression reading a resolved value path off the Settings, or `None`
/// for a path that has no scalar runtime value (a whole config/struct field).
fn value_expr(
    vp: &crate::codegen::entries::ValuePath<'_>,
    config: &CasingConfig,
) -> Option<String> {
    match (&vp.member, vp.target) {
        (None, Tref::Ref { .. }) | (None, Tref::Map(_, _)) | (None, Tref::List(_)) => None,
        (None, _) => Some(format!("s.{}", field_pascal(&vp.field.name, config))),
        (Some(member), t) => {
            if matches!(t, Tref::Ref { .. } | Tref::Map(_, _) | Tref::List(_)) {
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

/// The cast that makes a resolved value directly usable by the runtime's
/// value positions: integers widen to int64, branded strings flatten.
fn value_cast(t: &Tref, expr: &str) -> String {
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
/// always there.
fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
) -> Option<String> {
    if entry.is_guaranteed(vp.field) {
        return None;
    }
    Some(format!("{} == \"\"", why_var(&vp.field.name)))
}

/// The per-field resolution emitter: one field, one block of statements, in
/// the field's own idiom (scalar chain, switch, strict decode, composition).
struct Resolver<'a, 'b> {
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    config: &'a CasingConfig,
    helpers: &'b mut Helpers,
    refs: &'b mut Vec<Symbol>,
    body: &'b mut String,
}

impl Resolver<'_, '_> {
    fn push(&mut self, s: &str) {
        self.body.push_str(s);
    }

    fn import(&mut self, name: &str, module: &str) {
        self.refs.push(import(name, module));
    }

    fn ident(&self, name: &str) -> String {
        format!("s.{}", field_pascal(name, self.config))
    }

    /// The read expression of a sibling-field path (`creds.token` ->
    /// `s.Creds.Token`).
    fn path_expr(&self, path: &[String]) -> String {
        let mut out = "s".to_string();
        for seg in path {
            out.push('.');
            out.push_str(&field_pascal(seg, self.config));
        }
        out
    }

    fn path_type(&self, path: &[String]) -> Tref {
        let head = self
            .entry
            .fields
            .iter()
            .find(|f| path.first().is_some_and(|h| *h == f.name));
        let Some(head) = head else {
            return Tref::Prim(Prim::String);
        };
        if path.len() == 1 {
            return head.target.clone();
        }
        if let Tref::Ref { id, .. } = &head.target {
            if let Some(shape) = self.module.shapes.iter().find(|s| s.id == *id) {
                let target = match &shape.kind {
                    ShapeKind::Config { fields } => fields
                        .iter()
                        .find(|f| f.name == path[1])
                        .map(|f| f.target.clone()),
                    ShapeKind::Structure { members, .. } => members
                        .iter()
                        .find(|m| m.name == path[1])
                        .map(|m| m.target.clone()),
                    _ => None,
                };
                if let Some(t) = target {
                    return t;
                }
            }
        }
        Tref::Prim(Prim::String)
    }

    fn guaranteed(&self, name: &str) -> bool {
        self.entry
            .fields
            .iter()
            .find(|f| f.name == name)
            .is_some_and(|f| self.entry.is_guaranteed(f))
    }

    fn emit_field(&mut self, field: &EntryField) {
        match self.entry.field_shape(field, self.module) {
            FieldShape::Config(shape) => self.emit_config(field, shape),
            FieldShape::Structured(shape) => self.emit_structured(field, shape),
            FieldShape::Json => self.emit_json(field),
            FieldShape::Scalar => self.emit_scalar(field),
        }
    }

    /// A scalar field: an explicit `@arg`, a match selection, a `@format`
    /// derivation, or a plain source chain.
    fn emit_scalar(&mut self, field: &EntryField) {
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{} = {}\n", self.ident(&field.name), camel(&field.name));
            self.push(&assign);
            return;
        }
        if field.select.is_some() {
            self.emit_select(field);
            return;
        }
        if field.format.is_some() {
            self.emit_format(field);
            return;
        }
        self.emit_chain(field);
    }

    /// The plain source chain of one field, written to its Settings slot.
    fn emit_chain(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if self.entry.is_guaranteed(field) {
            let stmts = self.chain_cascade(field, &dest);
            self.push(&stmts);
        } else {
            let why = why_var(&field.name);
            let opener = format!("\t{why} := \"no source\"\n");
            self.push(&opener);
            let stmts = self.chain_sequential(field, &dest, &why);
            self.push(&stmts);
        }
    }

    /// A guaranteed chain as an if/else cascade ending in `@default`.
    fn chain_cascade(&mut self, field: &EntryField, dest: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            match source {
                Source::With => {
                    out.push_str(&format!(
                        "{}if w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t}}",
                        if first { "\t" } else { " else " },
                        carrier = camel(&field.name),
                    ));
                    first = false;
                }
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    out.push_str(&format!(
                        "{}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t}}",
                        if first { "\t" } else { " else " },
                        parse = self.env_parse(field, dest, &self.env_label(name)),
                    ));
                    first = false;
                }
                Source::Default(v) => {
                    let lit = literal(&field.target, v);
                    if first {
                        out.push_str(&format!("\t{dest} = {lit}\n"));
                    } else {
                        out.push_str(&format!(" else {{\n\t\t{dest} = {lit}\n\t}}\n"));
                    }
                    return out;
                }
                Source::Arg => {}
            }
        }
        if !first {
            out.push('\n');
        }
        out
    }

    /// A non-guaranteed chain as sequential "still absent" steps carrying the
    /// why-reason of the last source tried.
    fn chain_sequential(&mut self, field: &EntryField, dest: &str, why: &str) -> String {
        let mut out = String::new();
        let mut first = true;
        for source in &field.sources {
            let step = match source {
                Source::With => format!(
                    "if w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                    carrier = camel(&field.name),
                ),
                Source::Env(name) => {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    let miss = self.env_miss_reason(name);
                    let pre = self.env_name_prereq(name, why);
                    // The prereq ends in "else ", chaining straight into
                    // this if: the whole run is one balanced if/else-if/else.
                    format!(
                        "{pre}if v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = {miss}\n\t}}\n",
                        parse = indent(&self.env_parse(field, dest, &label)),
                    )
                }
                Source::Default(v) => format!(
                    "{dest} = {lit}\n\t{why} = \"\"\n",
                    lit = literal(&field.target, v),
                ),
                Source::Arg => continue,
            };
            if first {
                out.push_str(&format!("\t{step}"));
                first = false;
            } else {
                out.push_str(&format!(
                    "\tif {why} != \"\" {{\n{indented}\t}}\n",
                    indented = indent(step.trim_end_matches('\n')),
                ));
            }
        }
        out
    }

    /// The prereq guard when the env variable's own name comes from a sibling
    /// field that may itself be absent. Returns the opened guard text (the
    /// caller closes it).
    fn env_name_prereq(&self, name: &EnvName, why: &str) -> String {
        let EnvName::Field(fr) = name else {
            return String::new();
        };
        let Some(head) = fr.field.first() else {
            return String::new();
        };
        if self.guaranteed(head) {
            return String::new();
        }
        format!(
            "if {head_why} != \"\" {{\n\t\t{why} = \"{head} <- \" + {head_why}\n\t}} else ",
            head_why = why_var(head),
            head = head,
        )
    }

    fn env_lookup(&mut self, name: &EnvName) -> String {
        self.import("os", "os");
        match name {
            EnvName::Name(n) => format!("os.LookupEnv({n:?})"),
            EnvName::Field(fr) => {
                let expr = self.path_expr(&fr.field);
                let t = self.path_type(&fr.field);
                format!("os.LookupEnv({})", as_string(&expr, &t, self.helpers))
            }
        }
    }

    /// The label naming the variable in a parse error.
    fn env_label(&self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{n:?}"),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                let mut helpers = Helpers::default();
                as_string(&self.path_expr(&fr.field), &t, &mut helpers)
            }
        }
    }

    fn env_miss_reason(&mut self, name: &EnvName) -> String {
        match name {
            EnvName::Name(n) => format!("{:?}", format!("env {n}: empty")),
            EnvName::Field(fr) => {
                let t = self.path_type(&fr.field);
                format!(
                    "\"env \" + {} + \": empty\"",
                    as_string(&self.path_expr(&fr.field), &t, self.helpers)
                )
            }
        }
    }

    /// The statements parsing a raw env string `v` into the destination, by
    /// the field's declared type; a parse failure fails construction naming
    /// the variable and the type.
    fn env_parse(&mut self, field: &EntryField, dest: &str, label: &str) -> String {
        let t = &field.target;
        match t {
            Tref::Prim(Prim::Bool) => {
                self.import("fmt", "fmt");
                format!(
                    "\t\tswitch v {{\n\t\tcase \"true\", \"1\":\n\t\t\t{dest} = true\n\t\tcase \"false\", \"0\":\n\t\t\t{dest} = false\n\t\tdefault:\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid bool %q (want true/false/1/0)\", {label}, v)\n\t\t}}\n"
                )
            }
            Tref::Prim(p @ (Prim::I8 | Prim::I16 | Prim::I32 | Prim::I64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let bits = int_bits(p);
                format!(
                    "\t\tn, err := strconv.ParseInt(v, 10, {bits})\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n\t\t}}\n\t\t{dest} = {cast}(n)\n",
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(p @ (Prim::U8 | Prim::U16 | Prim::U32 | Prim::U64)) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                let bits = int_bits(p);
                format!(
                    "\t\tn, err := strconv.ParseUint(v, 10, {bits})\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid {prim} %q\", {label}, v)\n\t\t}}\n\t\t{dest} = {cast}(n)\n",
                    prim = prim_name(p),
                    cast = prim_spelling(p).go,
                )
            }
            Tref::Prim(Prim::Float) => {
                self.import("strconv", "strconv");
                self.import("fmt", "fmt");
                format!(
                    "\t\tn, err := strconv.ParseFloat(v, 64)\n\t\tif err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid float %q\", {label}, v)\n\t\t}}\n\t\t{dest} = n\n"
                )
            }
            Tref::Prim(Prim::Duration) => {
                self.import("time", "time");
                self.import("fmt", "fmt");
                format!(
                    "\t\tif _, err := time.ParseDuration(v); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: invalid duration %q\", {label}, v)\n\t\t}}\n\t\t{dest} = Duration(v)\n"
                )
            }
            _ => format!("\t\t{dest} = {}\n", cast_string(t, "v")),
        }
    }

    /// `field: T = match .subject { ... }` lowered to the native switch.
    fn emit_select(&mut self, field: &EntryField) {
        let Some(select) = field.select.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let guaranteed = self.entry.is_guaranteed(field);
        let why = why_var(&field.name);
        if !guaranteed {
            self.push(&format!("\t{why} := \"\"\n"));
        }
        let subject_head = select.subject.first().cloned().unwrap_or_default();
        let subject_expr = self.path_expr(&select.subject);
        let mut arms = String::new();
        let mut saw_wildcard = false;
        for arm in &select.arms {
            let stmts = self.arm_stmts(field, &arm.value, &dest, &why, guaranteed);
            match &arm.pattern {
                Some(p) => arms.push_str(&format!(
                    "\tcase {}:\n{}",
                    pattern_literal(p),
                    indent(stmts.trim_end_matches('\n'))
                )),
                None => {
                    saw_wildcard = true;
                    arms.push_str(&format!(
                        "\tdefault:\n{}",
                        indent(stmts.trim_end_matches('\n'))
                    ));
                }
            }
        }
        if !saw_wildcard {
            // Every enum is open, so an undeclared value can still arrive at
            // run time even with total declared coverage. Failing construction
            // beats freezing a silent zero value into the resolved settings.
            let miss = if guaranteed {
                self.import("fmt", "fmt");
                format!(
                    "\t\treturn nil, fmt.Errorf(\"{field}: match on {subject}: unmatched value %v\", {subject_expr})\n",
                    field = field.name,
                    subject = subject_head,
                )
            } else {
                format!("\t\t{why} = \"match: unmatched value\"\n")
            };
            arms.push_str(&format!("\tdefault:\n{miss}"));
        }
        let switch = format!("\tswitch {subject_expr} {{\n{arms}\t}}\n");
        if !self.guaranteed(&subject_head) {
            self.push(&format!(
                "\tif {subj_why} != \"\" {{\n\t\t{why} = \"{subject_head} <- \" + {subj_why}\n\t}} else {{\n{sw}\t}}\n",
                subj_why = why_var(&subject_head),
                sw = indent(switch.trim_end_matches('\n')),
            ));
        } else {
            self.push(&switch);
        }
    }

    fn arm_stmts(
        &mut self,
        field: &EntryField,
        value: &ArmValue,
        dest: &str,
        why: &str,
        guaranteed: bool,
    ) -> String {
        match value {
            ArmValue::Lit(v) => format!("\t{dest} = {}\n", literal(&field.target, v)),
            ArmValue::Field(path) => {
                let head = path.first().cloned().unwrap_or_default();
                let expr = self.path_expr(path);
                if self.guaranteed(&head) {
                    format!("\t{dest} = {expr}\n")
                } else {
                    format!(
                        "\tif {head_why} != \"\" {{\n\t\t{why} = \"{head} <- \" + {head_why}\n\t}} else {{\n\t\t{dest} = {expr}\n\t}}\n",
                        head_why = why_var(&head),
                    )
                }
            }
            ArmValue::Sources(sources) => {
                let stub = EntryField {
                    name: field.name.clone(),
                    target: field.target.clone(),
                    sources: sources.clone(),
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                };
                if guaranteed {
                    self.chain_cascade(&stub, dest)
                } else {
                    format!(
                        "\t{why} = \"no source\"\n{}",
                        self.chain_sequential(&stub, dest, why)
                    )
                }
            }
        }
    }

    /// `@format` template plus the `@str::*` transform pipeline.
    fn emit_format(&mut self, field: &EntryField) {
        let Some(format_parts) = field.format.clone() else {
            return;
        };
        let dest = self.ident(&field.name);
        let mut concat: Vec<String> = Vec::new();
        let mut absent_deps: Vec<String> = Vec::new();
        for part in &format_parts {
            match part {
                TemplatePart::Lit(s) => concat.push(format!("{s:?}")),
                TemplatePart::Field(p) => {
                    let head = p.first().cloned().unwrap_or_default();
                    if !self.guaranteed(&head) && !absent_deps.contains(&head) {
                        absent_deps.push(head.clone());
                    }
                    let t = self.path_type(p);
                    let expr = self.path_expr(p);
                    concat.push(as_string(&expr, &t, self.helpers));
                }
                // An op-input placeholder cannot appear in a field template;
                // the frontend rejects it. Render empty defensively.
                TemplatePart::Input(_) => concat.push("\"\"".to_string()),
            }
        }
        if !field.transforms.is_empty() {
            self.import("strings", "strings");
        }
        let expr = apply_transforms(concat.join(" + "), &field.transforms, self.helpers);
        let assign = format!("\t{dest} = {}\n", cast_string(&field.target, &expr));
        if absent_deps.is_empty() {
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        let mut out = format!("\t{why} := \"\"\n");
        let mut chain = String::new();
        for (i, dep) in absent_deps.iter().enumerate() {
            chain.push_str(&format!(
                "{}if {dep_why} != \"\" {{\n\t\t{why} = \"{dep} <- \" + {dep_why}\n\t}}",
                if i == 0 { "\t" } else { " else " },
                dep_why = why_var(dep),
            ));
        }
        chain.push_str(&format!(" else {{\n\t{assign}\t}}\n"));
        out.push_str(&chain);
        self.push(&out);
    }

    /// A structured source: an explicit `@arg`/`@with` value passes typed, a
    /// JSON env value decodes strictly into the wire struct (required members
    /// checked by name, unknown fields rejected), and declared validation runs
    /// at construction. The error carries the variable's name as context.
    fn emit_structured(&mut self, field: &EntryField, shape: &Shape) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{dest} = {}\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        // Without @arg a structured field can never be guaranteed (@default
        // does not apply to it), so the chain is always why-tracked.
        let why = why_var(&field.name);
        self.push(&format!("\t{why} := \"no source\"\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let step = format!(
                "\tif w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                carrier = camel(&field.name),
            );
            self.push(&step);
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        self.import("os", "os");
        self.import("json", "encoding/json");
        self.import("fmt", "fmt");
        self.import("bytes", "bytes");
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let ty = type_ident_from_id(&shape.id);
        let mut required_checks = String::new();
        if let ShapeKind::Structure { members, .. } = &shape.kind {
            for m in members.iter().filter(|m| m.required) {
                required_checks.push_str(&format!(
                    "\t\tif _, ok := probe[{name:?}]; !ok {{\n\t\t\treturn nil, fmt.Errorf(\"%s: missing field {name}\", {label})\n\t\t}}\n",
                    name = m.name,
                ));
            }
        }
        let validate = if validation::shape_has_checks(shape) {
            let en = error_names();
            format!(
                "\t\tif vs := Validate{ty}(decoded); len(vs) > 0 {{\n\t\t\treturn nil, &{validation}{{Violations: vs}}\n\t\t}}\n",
                validation = en.validation,
            )
        } else {
            String::new()
        };
        let block = format!(
            "\tif raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \t\tvar probe map[string]json.RawMessage\n\
             \t\tif err := json.Unmarshal([]byte(raw), &probe); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             {required_checks}\
             \t\tdec := json.NewDecoder(bytes.NewReader([]byte(raw)))\n\
             \t\tdec.DisallowUnknownFields()\n\
             \t\tvar decoded {ty}\n\
             \t\tif err := dec.Decode(&decoded); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             {validate}\
             \t\t{dest} = decoded\n\
             \t\t{why} = \"\"\n\
             \t}} else {{\n\t\t{why} = {miss}\n\t}}\n"
        );
        // An explicit @with value wins: the decode runs only while unset.
        self.push(&format!(
            "\tif {why} != \"\" {{\n{inner}\t}}\n",
            inner = indent(block.trim_end_matches('\n')),
        ));
    }

    /// A map/list field: an explicit `@arg`/`@with` value passes typed, an env
    /// value decodes as JSON whole.
    fn emit_json(&mut self, field: &EntryField) {
        let dest = self.ident(&field.name);
        if field.sources.iter().any(|s| matches!(s, Source::Arg)) {
            let assign = format!("\t{dest} = {}\n", camel(&field.name));
            self.push(&assign);
            return;
        }
        let why = why_var(&field.name);
        self.push(&format!("\t{why} := \"no source\"\n"));
        if field.sources.iter().any(|s| matches!(s, Source::With)) {
            let step = format!(
                "\tif w.{carrier} != nil {{\n\t\t{dest} = *w.{carrier}\n\t\t{why} = \"\"\n\t}} else {{\n\t\t{why} = \"not configured\"\n\t}}\n",
                carrier = camel(&field.name),
            );
            self.push(&step);
        }
        let Some(Source::Env(name)) = field.sources.iter().find(|s| matches!(s, Source::Env(_)))
        else {
            return;
        };
        self.import("os", "os");
        self.import("json", "encoding/json");
        self.import("fmt", "fmt");
        let lookup = self.env_lookup(name);
        let label = self.env_label(name);
        let miss = self.env_miss_reason(name);
        let block = format!(
            "\tif raw, ok := {lookup}; ok && raw != \"\" {{\n\
             \t\tif err := json.Unmarshal([]byte(raw), &{dest}); err != nil {{\n\t\t\treturn nil, fmt.Errorf(\"%s: %v\", {label}, err)\n\t\t}}\n\
             \t\t{why} = \"\"\n\
             \t}} else {{\n\t\t{why} = {miss}\n\t}}\n"
        );
        self.push(&format!(
            "\tif {why} != \"\" {{\n{inner}\t}}\n",
            inner = indent(block.trim_end_matches('\n')),
        ));
    }

    /// A composed config: per member, an entry `@bind` wins over the member's
    /// own sources.
    fn emit_config(&mut self, field: &EntryField, shape: &Shape) {
        let ShapeKind::Config { fields } = &shape.kind else {
            return;
        };
        let ty = type_ident_from_id(&shape.id);
        let dest = self.ident(&field.name);
        let mut block = format!("\t{{\n\t\tvar composed {ty}\n");
        for member in fields {
            let member_dest = format!("composed.{}", field_pascal(&member.name, self.config));
            let bind = field.binds.iter().find(|b| b.field == member.name);
            let mut member_stmts = String::new();
            if let Some(bind) = bind {
                let head = bind.source.first().cloned().unwrap_or_default();
                let expr = self.path_expr(&bind.source);
                if self.guaranteed(&head) {
                    member_stmts.push_str(&format!("\t{member_dest} = {expr}\n"));
                } else {
                    // The bound entry value wins when resolved; otherwise the
                    // member falls back to its own sources.
                    member_stmts.push_str(&format!(
                        "\tif {head_why} == \"\" {{\n\t\t{member_dest} = {expr}\n\t}} else {{\n{fallback}\t}}\n",
                        head_why = why_var(&head),
                        fallback = indent(
                            self.member_sources_stmts(member, &member_dest).trim_end_matches('\n')
                        ),
                    ));
                }
            } else {
                member_stmts.push_str(&self.member_sources_stmts(member, &member_dest));
            }
            block.push_str(&indent(member_stmts.trim_end_matches('\n')));
        }
        block.push_str(&format!("\t\t{dest} = composed\n\t}}\n"));
        self.push(&block);
    }

    /// A config member's own source chain (only `@env`/`@default` can appear
    /// inside a config).
    fn member_sources_stmts(&mut self, member: &EntryField, dest: &str) -> String {
        let stub = EntryField {
            name: member.name.clone(),
            target: member.target.clone(),
            sources: member.sources.clone(),
            format: None,
            transforms: vec![],
            select: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        };
        let guaranteed = stub.sources.iter().any(|s| matches!(s, Source::Default(_)));
        if guaranteed {
            self.chain_cascade(&stub, dest)
        } else {
            // No reason tracking inside a composition: an absent member simply
            // keeps its zero value (the entry-level requires cover consumed
            // chains).
            let mut out = String::new();
            for source in &stub.sources {
                if let Source::Env(name) = source {
                    let lookup = self.env_lookup(name);
                    let label = self.env_label(name);
                    out.push_str(&format!(
                        "\tif v, ok := {lookup}; ok && v != \"\" {{\n{parse}\t}}\n",
                        parse = indent(self.env_parse(&stub, dest, &label).trim_end_matches('\n')),
                    ));
                }
            }
            out
        }
    }
}

fn int_bits(p: &Prim) -> u32 {
    match p {
        Prim::I8 | Prim::U8 => 8,
        Prim::I16 | Prim::U16 => 16,
        Prim::I32 | Prim::U32 => 32,
        _ => 64,
    }
}

fn prim_name(p: &Prim) -> &'static str {
    match p {
        Prim::I8 => "i8",
        Prim::I16 => "i16",
        Prim::I32 => "i32",
        Prim::I64 => "i64",
        Prim::U8 => "u8",
        Prim::U16 => "u16",
        Prim::U32 => "u32",
        Prim::U64 => "u64",
        _ => "value",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::rendered;
    use crate::ir::decode_model;

    /// The canonical entry fixture (config, every source kind, derivation,
    /// selection, composition, protocol refs), decoded off the shared schema
    /// so the emitter is exercised against the real wire shape.
    fn fixture_module() -> Module {
        let text = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ir-schema/fixtures/entries_client.json"
        ));
        let model = decode_model(text).expect("fixture decodes");
        model.modules.into_iter().next().expect("one module")
    }

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &go_casing()), &GoRules::default())
    }

    fn serde_text(module: &Module) -> String {
        rendered(&serde_decls(module, &go_casing()), &GoRules::default())
    }

    #[test]
    fn the_construction_surface_is_new_options_settings_and_the_mock_interface() {
        let module = fixture_module();
        let types = types_text(&module);
        // Settings carry every resolved field plus the transport slots.
        assert!(types.contains("type Settings struct {"));
        assert!(types.contains("\tHTTPClient *http.Client\n"));
        assert!(types.contains("\tTransport  tonohttp.Transport\n"));
        assert!(types.contains("\tHeaders    map[string]string\n"));
        // The config is a construction-only struct.
        assert!(types.contains("type Conf struct {"));
        // One functional option per @with field, unprefixed (single entry).
        assert!(types.contains("func WithClientName(v string) ClientOption {"));
        assert!(types.contains("func WithTimeout(v Duration) ClientOption {"));
        assert!(types.contains("func WithMaxRetries(v int32) ClientOption {"));
        // The mock interface has ctx first and the conformance assertion.
        assert!(types.contains("type ClientAPI interface {"));
        assert!(types.contains("SaveNote(ctx context.Context, input Note) (Note, error)"));
        assert!(types.contains("var _ ClientAPI = (*Client)(nil)"));

        let serde = serde_text(&module);
        assert!(serde.contains("func New(apiKey string, opts ...ClientOption) (*Client, error) {"));
    }

    #[test]
    fn the_resolution_follows_the_declared_chains() {
        let module = fixture_module();
        let serde = serde_text(&module);
        // @arg lands positionally; @with falls back to @default.
        assert!(serde.contains("s.APIKey = apiKey"));
        assert!(serde.contains("if w.clientName != nil {"));
        assert!(serde.contains("s.ClientName = \"demo\""));
        // @format with @str transforms.
        assert!(serde.contains("s.ClientKey = strUpperSnake(strings.TrimSpace(s.ClientName))"));
        assert!(serde.contains("s.EndpointEnv = \"ENDPOINT_\" + s.ClientKey + \"_V2\""));
        // A dynamic env name reads through the resolved field.
        assert!(serde.contains("os.LookupEnv(s.EndpointEnv)"));
        // The match lowers to a switch with the wildcard as default.
        assert!(serde.contains("switch s.EndpointVersion {"));
        assert!(serde.contains("case \"v1\":"));
        assert!(serde.contains("case \"legacy\":"));
        assert!(serde.contains("s.Endpoint = \"https://old.example.com\""));
        // An arm that reads an absent chain reports it at the point of use.
        assert!(serde.contains("endpointWhy = \"endpoint_v1 <- \" + endpointV1Why"));
        // @bind: the entry value feeds the composed member; the unbound member
        // keeps its own chain.
        assert!(serde.contains("composed.APIKey = s.APIKey"));
        assert!(serde.contains("composed.Region = \"us\""));
        // The resolved values freeze for the runtime's ref positions, ints
        // widened and durations in milliseconds.
        assert!(serde.contains("values[\"max_retries\"] = int64(s.MaxRetries)"));
        assert!(serde.contains("ms, err := durationMs(string(s.Timeout))"));
        assert!(serde.contains("values[\"settings.api_key\"] = s.Settings.APIKey"));
    }

    #[test]
    fn the_env_boundary_parses_by_type_naming_variable_and_type() {
        let mut module = fixture_module();
        // Give a field an env-sourced integer to exercise the parse path.
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                fields.push(EntryField {
                    name: "port".into(),
                    target: Tref::Prim(Prim::I32),
                    sources: vec![Source::Env(EnvName::Name("PORT".into()))],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
            }
        }
        let serde = serde_text(&module);
        assert!(serde.contains("strconv.ParseInt(v, 10, 32)"));
        assert!(serde.contains("fmt.Errorf(\"%s: invalid i32 %q\", \"PORT\", v)"));
    }

    #[test]
    fn a_multi_entry_module_prefixes_the_colliding_companions() {
        let mut module = fixture_module();
        let second = {
            let mut clone = module
                .shapes
                .iter()
                .find(|s| matches!(s.kind, ShapeKind::Entry { .. }))
                .cloned()
                .expect("entry");
            clone.id = "notes#admin".into();
            clone
        };
        module.shapes.push(second);
        let types = types_text(&module);
        assert!(types.contains("type ClientSettings struct {"));
        assert!(types.contains("type AdminSettings struct {"));
        assert!(types.contains("func WithClientClientName(v string) ClientOption {"));
        assert!(types.contains("func WithAdminClientName(v string) AdminOption {"));
        let serde = serde_text(&module);
        assert!(serde
            .contains("func NewClient(apiKey string, opts ...ClientOption) (*Client, error) {"));
        assert!(
            serde.contains("func NewAdmin(apiKey string, opts ...AdminOption) (*Admin, error) {")
        );
    }

    #[test]
    fn bound_hooks_wire_the_settings_bridge_and_the_transport_slots() {
        let mut module = fixture_module();
        module.extensions = vec![
            crate::ir::Extension {
                name: "client_init".into(),
                kind: crate::ir::ExtKind::Hook,
                signature: None,
                bindings: [("go".to_string(), "ext/go/init.go#InitSettings".to_string())]
                    .into_iter()
                    .collect(),
                conformance: None,
            },
            crate::ir::Extension {
                name: "before_request".into(),
                kind: crate::ir::ExtKind::Hook,
                signature: None,
                bindings: [("go".to_string(), "ext/go/auth.go#AddBearer".to_string())]
                    .into_iter()
                    .collect(),
                conformance: None,
            },
        ];
        let serde = serde_text(&module);
        // client_init runs over the resolved Settings, after sources and
        // before validation; a bespoke failure is a ContractError.
        assert!(serde.contains("func clientInitHook(s *Settings) error {"));
        assert!(serde.contains("if err := clientInitHook(&s); err != nil {"));
        assert!(serde.contains("ContractError{ContractName: \"client_init\", Cause: err}"));
        // The construction wires the transport slots and the resolved values.
        assert!(serde.contains(
            "tonohttp.New(tonohttp.Options{Client: s.HTTPClient, Transport: s.Transport, Headers: s.Headers, Values: values})"
        ));
        // before_request is handed to the runtime once per client.
        assert!(serde.contains("func beforeRequestHook(ctx context.Context, req tonohttp.CanonicalRequest) (tonohttp.CanonicalRequest, error) {"));
        assert!(serde.contains("hooks: &tonohttp.Hooks{BeforeRequest: beforeRequestHook}"));
        // The hook order lands in the emitted text: init before the requires.
        let init = serde.find("clientInitHook(&s)").unwrap();
        let require = serde.find("errors.New(\"endpoint <- \"").unwrap();
        assert!(init < require);
    }

    /// Attach an opaque descriptor to every entry op, standing in for the
    /// frontend's protocol pass (the schema fixture is pre-protocol).
    fn with_descriptors(mut module: Module) -> Module {
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
                for op in operations {
                    op.traits.push(crate::ir::Trait {
                        id: "wire_descriptor".into(),
                        value: serde_json::json!({"http_method": "POST", "uri": "/notes/{id}"}),
                    });
                }
            }
        }
        module
    }

    #[test]
    fn the_method_maps_the_raw_outcome_onto_the_taxonomy() {
        let module = with_descriptors(fixture_module());
        let serde = serde_text(&module);
        // The descriptor is embedded verbatim, an opaque blob.
        assert!(serde.contains("var saveNoteDescriptor = mustDescriptor("));
        assert!(serde.contains(
            "outcome, err := c.runtime.Execute(ctx, saveNoteDescriptor, record, c.hooks)"
        ));
        assert!(serde.contains("case tonohttp.OutcomeTransport:"));
        assert!(serde.contains("&TransportError{Cause: outcome.Cause}"));
        assert!(serde.contains("DecodeSaveNoteError(outcome.Status, []byte(outcome.Body))"));
        assert!(serde.contains("&DecodeError{Path: \"$\", Expected: \"Note\", Raw: outcome.Body}"));
    }

    #[test]
    fn a_dynamic_env_name_off_an_absent_chain_emits_balanced_braces() {
        // A non-guaranteed chain whose env name comes from a sibling that may
        // itself be absent: the emitted run must be one balanced
        // if/else-if/else (the else chains straight into the lookup).
        let mut module = fixture_module();
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                // naming is env-only (not guaranteed); reader looks its value up.
                fields.push(EntryField {
                    name: "naming".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![Source::Env(EnvName::Name("NAMING".into()))],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
                fields.push(EntryField {
                    name: "reader".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![Source::Env(EnvName::Field(crate::ir::FieldRef {
                        field: vec!["naming".into()],
                    }))],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
            }
        }
        let serde = serde_text(&module);
        assert!(serde.contains("readerWhy = \"naming <- \" + namingWhy"));
        let new_fn = serde
            .split("func New(")
            .nth(1)
            .and_then(|rest| rest.split("\nfunc ").next())
            .expect("New body");
        assert_eq!(
            new_fn.matches('{').count(),
            new_fn.matches('}').count(),
            "unbalanced braces in the generated constructor:\n{new_fn}"
        );
    }

    #[test]
    fn structured_sources_decode_strictly_and_honor_explicit_values() {
        let mut module = fixture_module();
        let creds = Shape {
            id: "notes#credentials".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![
                    Member {
                        name: "token".into(),
                        target: Tref::Prim(Prim::String),
                        required: true,
                        default: None,
                        constraints: vec![crate::ir::Constraint::Length {
                            min: Some(1),
                            max: None,
                        }],
                        traits: vec![],
                    },
                    Member {
                        name: "account_id".into(),
                        target: Tref::Prim(Prim::String),
                        required: true,
                        default: None,
                        constraints: vec![],
                        traits: vec![],
                    },
                ],
            },
            traits: vec![],
        };
        module.shapes.push(creds);
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                // @with layers over the env decode; a labels map decodes whole.
                fields.push(EntryField {
                    name: "creds".into(),
                    target: Tref::Ref {
                        id: "notes#credentials".into(),
                        args: vec![],
                    },
                    sources: vec![
                        Source::With,
                        Source::Env(EnvName::Name("SERVICE_CREDENTIALS".into())),
                    ],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
                fields.push(EntryField {
                    name: "labels".into(),
                    target: Tref::Map(
                        Box::new(Tref::Prim(Prim::String)),
                        Box::new(Tref::Prim(Prim::String)),
                    ),
                    sources: vec![Source::Env(EnvName::Name("SERVICE_LABELS".into()))],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                });
            }
        }
        let serde = serde_text(&module);
        // Strict decode: probe for required members, unknown fields rejected,
        // declared validation at construction, the env name as context.
        assert!(serde.contains("fmt.Errorf(\"%s: missing field token\", \"SERVICE_CREDENTIALS\")"));
        assert!(serde.contains("dec.DisallowUnknownFields()"));
        assert!(serde.contains("ValidateCredentials(decoded)"));
        // The explicit @with value wins: the decode runs only while unset.
        assert!(serde.contains("if w.creds != nil {"));
        assert!(serde.contains("if credsWhy != \"\" {"));
        // The whole-JSON map field decodes with its env name as context.
        assert!(serde.contains("json.Unmarshal([]byte(raw), &s.Labels)"));
        let types = types_text(&module);
        assert!(types.contains("func WithCreds(v Credentials) ClientOption {"));
    }

    #[test]
    fn a_total_select_without_wildcard_fails_construction_on_an_open_enum_value() {
        let mut module = fixture_module();
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                let mut choice = EntryField {
                    name: "choice".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                };
                choice.select = Some(crate::ir::Select {
                    subject: vec!["client_name".into()],
                    arms: vec![crate::ir::SelectArm {
                        pattern: Some(serde_json::json!("demo")),
                        value: crate::ir::ArmValue::Lit(serde_json::json!("d")),
                    }],
                });
                fields.push(choice);
            }
        }
        let serde = serde_text(&module);
        assert!(serde.contains(
            "return nil, fmt.Errorf(\"choice: match on client_name: unmatched value %v\", s.ClientName)"
        ));
    }

    #[test]
    fn an_operation_without_a_descriptor_stubs_with_a_contract_error() {
        // The schema fixture carries no wire_descriptor (it is the canonical
        // pre-protocol encoding), so its op method must be the bespoke stub.
        let module = fixture_module();
        let serde = serde_text(&module);
        assert!(!serde.contains("var saveNoteDescriptor"));
        assert!(serde.contains("errors.New(\"operation has no transport binding\")"));
    }
}

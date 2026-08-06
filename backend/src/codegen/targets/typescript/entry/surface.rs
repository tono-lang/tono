//! The entry type surface: the construction-only config interfaces, the
//! Settings and config-object interfaces, the descriptor constants, and
//! the hook wrappers.

use super::*;
use crate::codegen::targets::typescript::client::import_specifier;

/// The construction-only config interfaces (they never cross the wire, so the
/// regular type emission skips them).
pub(super) fn config_interfaces(module: &Module, config: &CasingConfig) -> Vec<Decl> {
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
                        "  {}: {};\n",
                        field_camel(&f.name, config),
                        ts_type(&f.target)
                    )
                })
                .collect();
            Some(Decl::raw(format!(
                "// {name} is a construction-only composition of the entry surface; it\n\
                 // never crosses the wire and is module-local (the SDK builds it).\n\
                 interface {name} {{\n{members}}}"
            )))
        })
        .collect()
}

/// The Settings interface: every resolved field plus the transport slots the
/// bespoke `client_init` hook may fill.
pub(super) fn settings_interface(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    entry_module: &Module,
) -> Decl {
    let mut fields = String::new();
    let mut refs = vec![support_symbol("HttpTransport")];
    for f in entry.declared() {
        fields.push_str(&format!(
            "{doc}  {}: {};\n",
            field_camel_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
            ts_type(&f.target),
            doc = field_doc(&f.traits, "  "),
        ));
        refs.extend(type_refs(&f.target, entry_module));
    }
    Decl::raw_with(
        format!(
            "// {settings} are the resolved construction values of the {entry} entry,\n\
             // handed to the client_init hook before validation: bespoke code may\n\
             // overwrite any field (bespoke wins) and set transport through the slots.\n\
             // Exactly one transport slot may be set: fetch (native) or transport\n\
             // (canonical). headers are the base request headers (bespoke auth writes\n\
             // here); a declared @header wins only where nothing else set the name.\n\
             export interface {settings} {{\n{fields}  fetch?: typeof fetch;\n  transport?: HttpTransport;\n  headers: Record<string, string>;\n}}",
            settings = n.settings,
            entry = entry.name,
        ),
        refs,
    )
}

/// The optional config object: one optional property per `@with` field.
pub(super) fn config_object_interface(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    entry_module: &Module,
) -> Vec<Decl> {
    let configurable = entry.with_fields();
    if configurable.is_empty() {
        return Vec::new();
    }
    let fields: String = configurable
        .iter()
        .map(|f| {
            format!(
                "{doc}  {}?: {};\n",
                field_camel_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
                ts_type(&f.target),
                doc = field_doc(&f.traits, "  "),
            )
        })
        .collect();
    let refs = configurable
        .iter()
        .flat_map(|f| type_refs(&f.target, entry_module))
        .collect();
    vec![Decl::raw_with(
        format!(
            "// {config} carries the optional (@with) construction values of {client}.\n\
             export interface {config} {{\n{fields}}}",
            config = n.config,
            client = n.client,
        ),
        refs,
    )]
}

pub(super) fn discriminator_name(n: &Names, op: &Shape) -> String {
    format!(
        "decode{}Error",
        pascal(&format!("{}{}", n.op_prefix, op_local_name(&op.id)))
    )
}

pub(super) fn method_name(op: &Shape, config: &CasingConfig) -> String {
    let rename = rename_of(&op.traits, LANG);
    transform(
        op_local_name(&op.id),
        SymbolKind::Method,
        config,
        rename.as_deref(),
    )
}

/// The discrimination functions for the entry's operations, named through the
/// entry rule. An operation whose body is a raw bespoke implementation gets the
/// code-only variant under the same name: its outcome carries no protocol
/// status to match on. A typed implementation needs none at all, since it
/// already returns declared errors as typed values.
pub(super) fn discriminator_decls_for(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    bound: &[BoundExtension<'_>],
) -> Vec<Decl> {
    use crate::codegen::targets::typescript::errors;
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .filter_map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            let name = discriminator_name(n, op);
            if crate::codegen::ops::wire_binding(op).is_some() {
                return Some(errors::discriminator_fn_named(&name, &ordered, module));
            }
            match crate::codegen::extensions::impl_binding(bound, &op.id) {
                Some(b) if b.raw => Some(errors::outcome_discriminator_fn_named(
                    &name, &ordered, module,
                )),
                _ => None,
            }
        })
        .collect()
}

/// The runtime-facing hook wrappers (`before_request`/`after_response`) an
/// entry's transport calls before/after the request.
pub(super) fn transport_hook_wrappers(bound: &[BoundExtension<'_>], module: &Module) -> Vec<Decl> {
    let en = error_names();
    let mut decls = Vec::new();
    for (slot, ty, wrapper) in [
        ("before_request", "HttpRequest", "wrapBeforeRequest"),
        ("after_response", "HttpResponse", "wrapAfterResponse"),
    ] {
        let Some(b) = hook_binding(bound, slot) else {
            continue;
        };
        decls.push(Decl::raw_with(
            format!(
                "async function {wrapper}(x: {ty}): Promise<{ty}> {{\n  try {{\n    return await {sym}(x);\n  }} catch (e) {{\n    // A declared SDK error passes through typed; anything else is bespoke\n    // failure surfaced as a ContractError.\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"{slot}\", e);\n  }}\n}}",
                sym = b.symbol,
                root = en.root,
                contract = en.contract,
            ),
            vec![
                Symbol::imported(b.symbol, import_specifier(b.module, &module.name), b.symbol),
                support_symbol(ty),
                module_symbol(&en.root, module),
                module_symbol(&en.contract, module),
            ],
        ));
    }
    if let Some(b) = hook_binding(bound, "on_error") {
        decls.push(Decl::raw_with(
            format!(
                "function wrapOnError(err: {root}): {root} {{\n  try {{\n    return {sym}(err);\n  }} catch (e) {{\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"on_error\", e);\n  }}\n}}",
                root = en.root,
                contract = en.contract,
                sym = b.symbol,
            ),
            vec![
                Symbol::imported(b.symbol, import_specifier(b.module, &module.name), b.symbol),
                module_symbol(&en.root, module),
                module_symbol(&en.contract, module),
            ],
        ));
    }
    decls
}

/// The `client_init` boundary wrapper for entries: the bespoke symbol mutates
/// the resolved Settings in place (single-entry modules only: the bespoke
/// symbol has one signature).
pub(super) fn client_init_wrapper(
    bound: &[BoundExtension<'_>],
    entries: &[EntryModel<'_>],
    multi: bool,
    module: &Module,
) -> Vec<Decl> {
    let Some(b) = hook_binding(bound, "client_init") else {
        return Vec::new();
    };
    if multi {
        return Vec::new();
    }
    let Some(entry) = entries.first() else {
        return Vec::new();
    };
    let en = error_names();
    let n = names(entry, multi);
    vec![Decl::raw_with(
        format!(
            "// The client_init bridge: bespoke code runs over the resolved Settings\n\
             // (bespoke wins) before validation.\n\
             export function wrapClientInit(settings: {settings}): void {{\n  try {{\n    {sym}(settings);\n  }} catch (e) {{\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"client_init\", e);\n  }}\n}}",
            settings = n.settings,
            sym = b.symbol,
            root = en.root,
            contract = en.contract,
        ),
        vec![
            Symbol::imported(b.symbol, import_specifier(b.module, &module.name), b.symbol),
            module_symbol(&en.root, module),
            module_symbol(&en.contract, module),
            // The resolved Settings are the entry's, so the bridge imports them
            // from its group.
            module_symbol(&n.settings, module),
        ],
    )]
}

pub(super) fn apply_transforms(
    expr: String,
    transforms: &[String],
    helpers: &mut Helpers,
) -> String {
    crate::codegen::entries::plan::apply_transforms(
        expr,
        transforms,
        &mut helpers.transforms,
        |t, out| match t {
            "trim" => Some(format!("({out}).trim()")),
            "lower" => Some(format!("({out}).toLowerCase()")),
            "upper" => Some(format!("({out}).toUpperCase()")),
            _ => None,
        },
        // The helper is imported by name, and TypeScript spells a function in
        // camelCase, so the canonical name is lowered at the first letter.
        |name| {
            let mut chars = name.chars();
            match chars.next() {
                Some(first) => first.to_lowercase().chain(chars).collect(),
                None => String::new(),
            }
        },
    )
}

/// The resolution helpers, which are pure string, environment and duration
/// work: they serve every entry of every module, so they live in the SDK's
/// shared group rather than beside any one of them. Emitted whole rather than
/// per use, so an entry group's imports do not depend on which transforms a
/// spec happens to name.
/// Reading an environment variable, which a declared source resolves from.
pub fn env_helpers() -> Vec<Decl> {
    let mut decls = Vec::new();
    {
        decls.push(Decl::raw_providing(
            "readEnv",
            "// readEnv treats an unset and an empty variable the same: empty means\n\
             // not set, per the declared-source contract.\n\
             export function readEnv(name: string): string | undefined {\n\
             \x20 const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;\n\
             \x20 const v = env?.[name];\n\
             \x20 return v === undefined || v === \"\" ? undefined : v;\n\
             }"
            .to_string(),
            Vec::new(),
        ));
    }
    decls
}

/// Parsing the duration spelling the targets share into milliseconds.
pub fn duration_helpers() -> Vec<Decl> {
    let mut decls = Vec::new();
    {
        decls.push(Decl::raw_providing(
            "durationToMs",
            "// durationToMs parses the duration spelling shared across targets\n\
             // (Go's ParseDuration grammar: optional sign, bare zero, unit runs)\n\
             // into the runtime's millisecond values.\n\
             export function durationToMs(v: string): number {\n\
             \x20 let rest = v;\n\
             \x20 let sign = 1;\n\
             \x20 if (rest.startsWith(\"-\")) {\n\
             \x20   sign = -1;\n\
             \x20   rest = rest.slice(1);\n\
             \x20 } else if (rest.startsWith(\"+\")) {\n\
             \x20   rest = rest.slice(1);\n\
             \x20 }\n\
             \x20 if (rest === \"0\") {\n\
             \x20   return 0;\n\
             \x20 }\n\
             \x20 // The number grammar matches Go's: digits, digits.digits,\n\
             \x20 // trailing-dot (\"1.s\") and leading-dot (\".5s\") forms.\n\
             \x20 // Sticky alone: exec advances lastIndex, and y already anchors\n\
             \x20 // each match to it (g would be redundant).\n\
             \x20 // Both micro signs Go accepts: U+00B5 (micro) and U+03BC (Greek mu).\n\
             \x20 const re = /(\\d+(?:\\.\\d*)?|\\.\\d+)(ns|us|\\u00b5s|\\u03bcs|ms|s|m|h)/y;\n\
             \x20 const unit: Record<string, number> = { ns: 1e-6, us: 1e-3, \"\\u00b5s\": 1e-3, \"\\u03bcs\": 1e-3, ms: 1, s: 1000, m: 60000, h: 3600000 };\n\
             \x20 let total = 0;\n\
             \x20 let consumed = 0;\n\
             \x20 for (let m = re.exec(rest); m !== null; m = re.exec(rest)) {\n\
             \x20   total += Number(m[1]) * unit[m[2]];\n\
             \x20   consumed = re.lastIndex;\n\
             \x20 }\n\
             \x20 if (consumed !== rest.length || rest.length === 0) {\n\
             \x20   throw new Error(`invalid duration ${JSON.stringify(v)}`);\n\
             \x20 }\n\
             \x20 return sign * total;\n\
             }"
            .to_string(),
            Vec::new(),
        ));
    }
    decls
}

/// The casing transforms an `@str::` pipeline lowers to.
pub fn casing_helpers() -> Vec<Decl> {
    let mut decls = Vec::new();
    {
        decls.push(Decl::raw_providing(
            "strTransformWords",
            "// strTransformWords splits a resolved value for the casing transforms:\n\
             // runs of spaces, hyphens, and underscores separate words.\n\
             export function strTransformWords(s: string): string[] {\n\
             \x20 return s.split(/[ _-]+/).filter((w) => w !== \"\");\n\
             }"
            .to_string(),
            Vec::new(),
        ));
        for t in ["upper_snake", "snake", "kebab", "pascal"] {
            let (name, body) = match t {
                "upper_snake" => (
                    "strUpperSnake",
                    "  return strTransformWords(s).map((w) => w.toUpperCase()).join(\"_\");",
                ),
                "snake" => (
                    "strSnake",
                    "  return strTransformWords(s).map((w) => w.toLowerCase()).join(\"_\");",
                ),
                "kebab" => (
                    "strKebab",
                    "  return strTransformWords(s).map((w) => w.toLowerCase()).join(\"-\");",
                ),
                "pascal" => (
                    "strPascal",
                    "  return strTransformWords(s).map((w) => w.charAt(0).toUpperCase() + w.slice(1).toLowerCase()).join(\"\");",
                ),
                _ => continue,
            };
            decls.push(Decl::raw_providing(
                name,
                format!("export function {name}(s: string): string {{\n{body}\n}}"),
                Vec::new(),
            ));
        }
    }
    decls
}

/// Every resolution helper, for a caller that wants them as one list.
pub fn resolution_helpers() -> Vec<Decl> {
    let mut decls = env_helpers();
    decls.extend(duration_helpers());
    decls.extend(casing_helpers());
    decls
}

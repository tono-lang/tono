//! The entry type surface: the construction-only config interfaces, the
//! Settings and config-object interfaces, the descriptor constants, and
//! the hook wrappers.

use super::*;

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
    let mut refs = vec![runtime_import("CanonicalTransport")];
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
             export interface {settings} {{\n{fields}  fetch?: typeof fetch;\n  transport?: CanonicalTransport;\n  headers: Record<string, string>;\n}}",
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

pub(super) fn descriptor_var(n: &Names, op: &Shape) -> String {
    camel(&format!(
        "{}{}_descriptor",
        n.op_prefix,
        op_local_name(&op.id)
    ))
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

/// The JSON descriptor embedded as a JavaScript string literal the runtime
/// parses at load (encode twice: an opaque blob, no field ever read).
pub(super) fn embed(descriptor: &serde_json::Value) -> String {
    let json = serde_json::to_string(descriptor).unwrap_or_else(|_| "null".into());
    serde_json::to_string(&json).unwrap_or_else(|_| "\"null\"".into())
}

pub(super) fn descriptor_decls(entry: &EntryModel<'_>, n: &Names) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter_map(|op| {
            let descriptor = wire_descriptor(op)?;
            Some(Decl::raw_with(
                format!(
                    "const {var}: WireDescriptor = JSON.parse({literal});",
                    var = descriptor_var(n, op),
                    literal = embed(descriptor),
                ),
                vec![runtime_import("WireDescriptor")],
            ))
        })
        .collect()
}

/// The discrimination functions for the entry's operations, named through the
/// entry rule.
pub(super) fn discriminator_decls_for(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            crate::codegen::targets::typescript::errors::discriminator_fn_named(
                &discriminator_name(n, op),
                &ordered,
                module,
            )
        })
        .collect()
}

/// The runtime-facing hook wrappers (`before_request`/`after_response`),
/// shared spelling with the loose-op client.
pub(super) fn transport_hook_wrappers(bound: &[BoundExtension<'_>], module: &Module) -> Vec<Decl> {
    let en = error_names();
    let mut decls = Vec::new();
    for (slot, ty, wrapper) in [
        ("before_request", "CanonicalRequest", "wrapBeforeRequest"),
        ("after_response", "CanonicalResponse", "wrapAfterResponse"),
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
                Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
                runtime_import(ty),
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
                Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
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
             function wrapClientInit(settings: {settings}): void {{\n  try {{\n    {sym}(settings);\n  }} catch (e) {{\n    if (e instanceof {root}) throw e;\n    throw new {contract}(\"client_init\", e);\n  }}\n}}",
            settings = n.settings,
            sym = b.symbol,
            root = en.root,
            contract = en.contract,
        ),
        vec![
            Symbol::imported(b.symbol, import_specifier(b.module), b.symbol),
            module_symbol(&en.root, module),
            module_symbol(&en.contract, module),
        ],
    )]
}

/// A bound file path as a TypeScript import specifier (mirrors the loose-op
/// client's rule).
pub(super) fn import_specifier(module: &str) -> String {
    let path = module
        .strip_suffix(".ts")
        .or_else(|| module.strip_suffix(".tsx"))
        .unwrap_or(module);
    if path.starts_with('.') || path.starts_with('/') {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

pub(super) fn helper_decls(helpers: &Helpers) -> Vec<Decl> {
    let mut decls = Vec::new();
    if helpers.read_env {
        decls.push(Decl::raw(
            "// readEnv treats an unset and an empty variable the same: empty means\n\
             // not set, per the declared-source contract.\n\
             function readEnv(name: string): string | undefined {\n\
             \x20 const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;\n\
             \x20 const v = env?.[name];\n\
             \x20 return v === undefined || v === \"\" ? undefined : v;\n\
             }"
            .to_string(),
        ));
    }
    if helpers.duration_ms {
        decls.push(Decl::raw(
            "// durationToMs parses the duration spelling shared across targets\n\
             // (Go's ParseDuration grammar: optional sign, bare zero, unit runs)\n\
             // into the runtime's millisecond values.\n\
             function durationToMs(v: string): number {\n\
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
        ));
    }
    if !helpers.transforms.is_empty() {
        decls.push(Decl::raw(
            "// strTransformWords splits a resolved value for the casing transforms:\n\
             // runs of spaces, hyphens, and underscores separate words.\n\
             function strTransformWords(s: string): string[] {\n\
             \x20 return s.split(/[ _-]+/).filter((w) => w !== \"\");\n\
             }"
            .to_string(),
        ));
        for t in &helpers.transforms {
            let (name, body) = match *t {
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
            decls.push(Decl::raw(format!(
                "function {name}(s: string): string {{\n{body}\n}}"
            )));
        }
    }
    decls
}

pub(super) fn apply_transforms(
    expr: String,
    transforms: &[String],
    helpers: &mut Helpers,
) -> String {
    let mut out = expr;
    for t in transforms {
        out = match t.as_str() {
            "trim" => format!("({out}).trim()"),
            "lower" => format!("({out}).toLowerCase()"),
            "upper" => format!("({out}).toUpperCase()"),
            other => match crate::codegen::entries::plan::casing_transform(other, &out) {
                Some((key, expr)) => {
                    helpers.transforms.insert(key);
                    expr
                }
                None => out,
            },
        };
    }
    out
}

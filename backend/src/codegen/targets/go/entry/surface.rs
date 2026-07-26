//! The entry type surface: the construction-only config structs, the
//! resolved Settings, the functional options, the client struct and its
//! mock interface.

use super::*;

/// The construction-only config structs. They never cross the wire, so the
/// regular type emission skips them; the entry surface is what makes them
/// real (as plain structs the resolved `Settings` embeds).
pub(super) fn config_structs(module: &Module, config: &CasingConfig) -> Vec<Decl> {
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
                        "{doc}\t{} {}\n",
                        field_pascal(&f.name, config),
                        go_type(&f.target),
                        doc = field_doc(&f.traits, "\t"),
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
pub(super) fn settings_decl(entry: &EntryModel<'_>, n: &Names, config: &CasingConfig) -> Decl {
    let mut fields = String::new();
    for f in entry.declared() {
        fields.push_str(&format!(
            "{doc}\t{} {}\n",
            field_pascal(&f.name, config),
            go_type(&f.target),
            doc = field_doc(&f.traits, "\t"),
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
pub(super) fn option_decls(entry: &EntryModel<'_>, n: &Names, multi: bool) -> Vec<Decl> {
    let configurable = entry.with_fields();
    if configurable.is_empty() {
        return Vec::new();
    }
    let mut decls = Vec::new();
    let carrier_fields: String = configurable
        .iter()
        .map(|f| format!("\t{} *{}\n", camel(&f.name), go_type(&f.target)))
        .collect();
    decls.push(Decl::raw(format!(
        "// {option} configures an optional (@with) construction value of {client}.\n\
         type {option} func(*{carrier})\n\n\
         type {carrier} struct {{\n{carrier_fields}}}",
        option = n.option,
        client = n.client,
        carrier = n.carrier,
    )));
    for f in configurable {
        let fn_name = pascal(&format!(
            "with_{}",
            companion_name(entry.name, &f.name, multi)
        ));
        decls.push(Decl::raw(format!(
            "{doc}// {fn_name} sets the {field} construction value.\n\
             func {fn_name}(v {ty}) {option} {{\n\treturn func(w *{carrier_ty}) {{ w.{member} = &v }}\n}}",
            field = f.name,
            ty = go_type(&f.target),
            option = n.option,
            carrier_ty = n.carrier,
            member = camel(&f.name),
            doc = field_doc(&f.traits, ""),
        )));
    }
    decls
}

/// The client struct, its mock interface (one method per operation, `ctx`
/// first), and the compile-time conformance assertion.
pub(super) fn client_decls(entry: &EntryModel<'_>, n: &Names, config: &CasingConfig) -> Vec<Decl> {
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
pub(super) fn method_signature(op: &Shape, config: &CasingConfig) -> (String, Vec<Symbol>) {
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

pub(super) fn method_name(op: &Shape, config: &CasingConfig) -> String {
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

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
            let name = config_type_ident(&shape.id);
            // A member's type can be one the SDK shares (a branded well-known),
            // which is a different package: the text names it, so the reference
            // has to be declared or the import is not collected.
            let mut refs = Vec::new();
            let members: String = fields
                .iter()
                .map(|f| {
                    push_type_symbols(&f.target, &mut refs);
                    format!(
                        "{doc}\t{} {}\n",
                        field_pascal(&f.name, config),
                        go_type(&f.target),
                        doc = field_doc(&f.traits, "\t"),
                    )
                })
                .collect();
            Some(Decl::raw_with(
                format!(
                    "// {name} is a construction-only composition of the entry surface; it\n\
                     // never crosses the wire and is unexported (the SDK builds it).\n\
                     type {name} struct {{\n{members}}}"
                ),
                refs,
            ))
        })
        .collect()
}

/// The `Settings` struct: every resolved entry field plus the transport slots
/// (native client or canonical transport) and the base request headers.
pub(super) fn settings_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    module: &Module,
) -> Decl {
    let mut fields = String::new();
    let mut refs = vec![
        import("http", "net/http"),
        super::support_symbol("HTTPTransport"),
    ];
    for f in entry.declared() {
        push_field_type_symbols(&f.target, module, &mut refs);
        fields.push_str(&format!(
            "{doc}\t{} {}\n",
            entry_field_ident(entry, module, config, &f.name),
            field_go_type_storage(&f.target, module),
            doc = field_doc(&f.traits, "\t"),
        ));
    }
    let text = format!(
        "// {settings} are the resolved construction values of the {entry} entry.\n\
         // Exactly one transport slot may be set: HTTPClient (native) or Transport\n\
         // (canonical). Headers are the base request headers; a declared @header\n\
         // wins only where nothing else set the name.\n\
         type {settings} struct {{\n{fields}\n\tHTTPClient *http.Client\n\tTransport  {transport}\n\tHeaders    map[string]string\n}}",
        settings = n.settings,
        entry = entry.name,
        transport = shared_slot("HTTPTransport"),
    );
    Decl::raw_with(text, refs)
}

/// The functional-option surface: one `With*` per `@with` field over a private
/// carrier struct (a pointer per field, so an unset option is distinguishable
/// from a zero value).
pub(super) fn option_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    multi: bool,
    module: &Module,
) -> Vec<Decl> {
    let configurable = entry.with_fields();
    if configurable.is_empty() {
        return Vec::new();
    }
    let mut decls = Vec::new();
    let mut carrier_refs = Vec::new();
    let carrier_fields: String = configurable
        .iter()
        .map(|f| {
            push_field_type_symbols(&f.target, module, &mut carrier_refs);
            // A foreign opaque handle's Go type is already a pointer (a
            // guess at the real package's return convention), so
            // the carrier holds it directly; every other `@with` value gets
            // the usual extra pointer so an unset option is distinguishable
            // from a zero value.
            let carrier_ty = if is_foreign_handle(&f.target, module) {
                field_go_type_storage(&f.target, module)
            } else {
                format!("*{}", field_go_type(&f.target, module))
            };
            format!("\t{} {carrier_ty}\n", camel(&f.name))
        })
        .collect();
    decls.push(Decl::raw_with(
        format!(
            "// {option} configures an optional (@with) construction value of {client}.\n\
         type {option} func(*{carrier})\n\n\
         type {carrier} struct {{\n{carrier_fields}}}",
            option = n.option,
            client = n.client,
            carrier = n.carrier,
        ),
        carrier_refs,
    ));
    for f in configurable {
        let fn_name = with_option_name(entry.name, f, multi);
        // godoc reads a doc comment as documentation only when it opens with the
        // declared identifier, so the canonical sentence leads and any @doc /
        // @deprecated lines follow as continuation.
        let mut option_refs = Vec::new();
        push_field_type_symbols(&f.target, module, &mut option_refs);
        let ty = field_go_type(&f.target, module);
        let assign = match ext::foreign_handle(&f.target, module) {
            // The real concrete value never satisfies the field's own
            // generated interface directly (its methods return the foreign
            // package's own types, not the logical ones the interface
            // declares), so it is wrapped in the matching adapter, exactly
            // like the field's own real construction call is. The setter's
            // own parameter spells the real concrete type (unlike the
            // carrier field it feeds), so it needs the library's own import,
            // which the storage-typed `push_field_type_symbols` above never
            // pulls in.
            Some((lib, type_name)) => {
                if let Some(sym) = ext::handle_symbol(lib) {
                    option_refs.push(sym);
                }
                format!(
                    "w.{member} = &{adapter}{{real: v}}",
                    member = camel(&f.name),
                    adapter = ext::handle_adapter_ident(&lib.name, &type_name),
                )
            }
            None => format!("w.{member} = &v", member = camel(&f.name)),
        };
        decls.push(Decl::raw_with(
            format!(
                "// {fn_name} sets the {field} construction value.\n\
             {doc}func {fn_name}(v {ty}) {option} {{\n\treturn func(w *{carrier_ty}) {{ {assign} }}\n}}",
                field = f.name,
                option = n.option,
                carrier_ty = n.carrier,
                doc = field_doc(&f.traits, ""),
            ),
            option_refs,
        ));
    }
    decls
}

fn is_foreign_handle(t: &Tref, module: &Module) -> bool {
    ext::foreign_handle(t, module).is_some()
}

/// The client struct, its mock interface (one method per operation, `ctx`
/// first), and the compile-time conformance assertion. Besides the resolved
/// settings the struct carries only what this entry's operations declare: one
/// pre-converted field per distinct `@timeout` path, and the retry backoff's
/// timing seam.
pub(super) fn client_decls(entry: &EntryModel<'_>, n: &Names, config: &CasingConfig) -> Vec<Decl> {
    let mut methods = String::new();
    // An entry with no operations declares an empty mock interface, which
    // never spells `context.Context` anywhere in its own text; importing it
    // unconditionally would leave that file with an unused import.
    let mut refs = if entry.operations.is_empty() {
        Vec::new()
    } else {
        vec![import("context", "context")]
    };
    for op in entry.operations {
        let (sig, sig_refs) = method_signature(op, config);
        refs.extend(sig_refs);
        methods.push_str(&format!("\t{sig}\n"));
    }
    let mut struct_fields = format!("\tsettings {settings}\n", settings = n.settings);
    let mut struct_refs = Vec::new();
    for path in timeout_paths(entry).values() {
        struct_fields.push_str(&format!(
            "\t// {ident} is the operation @timeout, converted once at construction.\n\
             \t{ident} time.Duration\n",
            ident = super::timeout_field_ident(entry, config, path),
        ));
        struct_refs.push(import("time", "time"));
    }
    if entry_has_retry(entry) {
        struct_fields.push_str(&format!(
            "\t// timing is the clock behind the retry backoff; a test in this package\n\
             \t// may pin it. The zero value uses the real clock and jitter.\n\
             \ttiming {}\n",
            shared_slot("Timing"),
        ));
        struct_refs.push(super::shared_symbol("Timing"));
    }
    let doc = doc_of(&entry.shape.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    vec![
        Decl::raw_with(
            format!(
                "{doc}// {client} is the generated SDK client the {entry} entry declares.\n\
                 type {client} struct {{\n{struct_fields}}}",
                client = n.client,
                entry = entry.name,
            ),
            struct_refs,
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

/// The distinct `@timeout` field paths this entry's operations declare, keyed
/// by their dotted spelling (usually one, but a multi-op entry could bind more
/// than one).
pub(super) fn timeout_paths(
    entry: &EntryModel<'_>,
) -> std::collections::BTreeMap<String, Vec<String>> {
    entry
        .operations
        .iter()
        .filter_map(|op| match &op.kind {
            ShapeKind::Operation { wire: Some(w), .. } => {
                w.timeout.as_ref().map(|p| (p.join("."), p.clone()))
            }
            _ => None,
        })
        .collect()
}

/// Whether any of the entry's operations declares `@retry`, which is what puts
/// the timing seam on the client.
pub(super) fn entry_has_retry(entry: &EntryModel<'_>) -> bool {
    entry.operations.iter().any(
        |op| matches!(&op.kind, ShapeKind::Operation { wire: Some(w), .. } if w.retry.is_some()),
    )
}

/// The public functional-option name of a `@with` field, honoring
/// `@rename(go)`; the carrier member stays the plain camel name (it is
/// internal and read the same way in resolve).
pub(super) fn with_option_name(entry_name: &str, f: &EntryField, multi: bool) -> String {
    let display = rename_of(&f.traits, LANG).unwrap_or_else(|| f.name.clone());
    pascal(&format!(
        "with_{}",
        companion_name(entry_name, &display, multi)
    ))
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

/// One entry's type surface: its resolved Settings, its functional options, and
/// its client struct with the mock interface.
pub(super) fn entry_type_decls(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    config: &CasingConfig,
    multi: bool,
) -> Vec<Decl> {
    let mut decls = vec![settings_decl(entry, n, config, module)];
    decls.extend(option_decls(entry, n, multi, module));
    decls.extend(client_decls(entry, n, config));
    decls
}

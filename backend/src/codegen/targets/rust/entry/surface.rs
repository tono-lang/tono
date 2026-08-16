//! The entry type surface: the construction-only config structs, the
//! `Settings` struct, and the discriminator / method naming shared by
//! `mod.rs`.

use super::*;

/// The construction-only config structs. They never cross the wire, so the
/// regular type emission (`rust/types.rs::emit_type`) skips
/// `ShapeKind::Config`; the entry surface is what makes them real, as plain
/// crate-local structs the resolved `Settings` composes into via `@bind`.
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
                        "    pub {}: {},\n",
                        field_snake(&f.name, config),
                        rust_type(&f.target)
                    )
                })
                .collect();
            Some(Decl::raw(format!(
                "/// {name} is a construction-only composition of the entry surface; it\n\
                 /// never crosses the wire and is crate-local (the SDK builds it).\n\
                 #[derive(Clone, Debug)]\n\
                 pub(crate) struct {name} {{\n{members}}}"
            )))
        })
        .collect()
}

/// The `Settings` struct: every resolved entry field plus the transport
/// slots.
pub(super) fn settings_struct_decl(
    entry: &EntryModel<'_>,
    n: &Names,
    config: &CasingConfig,
    module: &Module,
) -> Decl {
    let mut fields = String::new();
    let mut refs = Vec::new();
    for f in entry.declared() {
        fields.push_str(&format!(
            "{doc}    pub {}: {},\n",
            field_snake_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), config),
            rust_type(&f.target),
            doc = field_doc(&f.traits, "    "),
        ));
        push_type_symbols(&f.target, &module.name, &mut refs);
    }
    refs.push(support_symbol("HttpTransport"));
    let text = format!(
        "/// {settings} are the resolved construction values of the {entry} entry.\n\
         /// Exactly one transport slot may be set: `client` (native `reqwest`,\n\
         /// present only with the crate's default-on `reqwest` feature) or\n\
         /// `transport` (canonical). `headers` are the base request headers; a\n\
         /// declared `@header` wins only where nothing else set the name.\n\
         pub(crate) struct {settings} {{\n\
         {fields}    #[cfg(feature = \"reqwest\")]\n\
         \x20   pub client: Option<reqwest::Client>,\n\
         \x20   pub transport: Option<HttpTransport>,\n\
         \x20   pub headers: std::collections::HashMap<String, String>,\n\
         }}",
        settings = n.settings,
        entry = entry.name,
    );
    Decl::raw_with(text, refs)
}

pub(super) fn discriminator_fn_name(n: &Names, op: &Shape) -> String {
    snake(&format!(
        "decode_{}{}_error",
        n.op_prefix,
        op_local_name(&op.id)
    ))
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

/// The discrimination functions for the entry's operations, named through
/// the entry rule. A wire-bound operation gets the status-and-body variant;
/// an operation whose body is a raw bespoke implementation
/// (`impl_op::method`'s raw branch) gets the code-only variant, since a
/// bespoke outcome carries no protocol status.
pub(super) fn discriminator_decls_for(
    entry: &EntryModel<'_>,
    n: &Names,
    module: &Module,
    bound: &[BoundExtension<'_>],
) -> Vec<Decl> {
    use crate::codegen::targets::rust::errors;
    entry
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .filter_map(|op| {
            let ordered = crate::codegen::ops::discrimination_order(op, module);
            let name = discriminator_fn_name(n, op);
            if wire_binding(op).is_some() {
                return Some(errors::discriminator_fn_named(&name, &ordered));
            }
            match impl_binding(bound, &op.id) {
                Some(b) if b.raw => Some(errors::outcome_discriminator_fn_named(&name, &ordered)),
                _ => None,
            }
        })
        .collect()
}

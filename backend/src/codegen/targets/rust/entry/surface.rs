//! The entry type surface: the construction-only config structs, the
//! `Settings` struct, the embedded op descriptors, and the discriminator /
//! method naming shared by `mod.rs`.

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
/// slots the bespoke `client_init` hook may fill.
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
    let text = format!(
        "/// {settings} are the resolved construction values of the {entry} entry:\n\
         /// bespoke `client_init` code may overwrite any field (bespoke wins) and\n\
         /// set transport through the slots. Exactly one transport slot may be\n\
         /// set: `client` (native) or `transport` (canonical). `headers` are the\n\
         /// base request headers (bespoke auth writes here); a declared\n\
         /// `@header` wins only where nothing else set the name.\n\
         pub(crate) struct {settings} {{\n\
         {fields}    pub client: Option<reqwest::Client>,\n\
         \x20   pub transport: Option<tono_http_runtime::Transport>,\n\
         \x20   pub headers: std::collections::HashMap<String, String>,\n\
         }}",
        settings = n.settings,
        entry = entry.name,
    );
    Decl::raw_with(text, refs)
}

/// The Rust `"..."` literal for `s`, escaping only what Rust's string-literal
/// grammar requires (JSON's own escaping is not directly Rust syntax: its
/// `\uXXXX` form lacks the braces Rust's `\u{..}` needs), so a descriptor
/// carrying any control character still embeds correctly rather than relying
/// on the two escaping grammars happening to agree.
fn rust_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{{{:x}}}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(super) fn descriptor_fn_name(n: &Names, op: &Shape) -> String {
    snake(&format!(
        "{}{}_descriptor",
        n.op_prefix,
        op_local_name(&op.id)
    ))
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

/// One function per operation, parsing its embedded descriptor literal once
/// (a `OnceLock`, stable since Rust 1.70) and caching it.
pub(super) fn descriptor_decls(entry: &EntryModel<'_>, n: &Names) -> Vec<Decl> {
    entry
        .operations
        .iter()
        .filter_map(|op| {
            let descriptor = wire_descriptor(op)?;
            let json = serde_json::to_string(descriptor).unwrap_or_else(|_| "null".into());
            let fn_name = descriptor_fn_name(n, op);
            Some(Decl::raw(format!(
                "fn {fn_name}() -> &'static tono_http_runtime::WireDescriptor {{\n\
                 \x20   static DESCRIPTOR: std::sync::OnceLock<tono_http_runtime::WireDescriptor> = std::sync::OnceLock::new();\n\
                 \x20   DESCRIPTOR.get_or_init(|| tono_http_runtime::WireDescriptor::parse({literal}).expect(\"valid wire descriptor\"))\n\
                 }}",
                literal = rust_string_literal(&json),
            )))
        })
        .collect()
}

/// The discrimination functions for the entry's operations, named through
/// the entry rule. A descriptor-bound operation gets the status-and-body
/// variant; an operation whose body is a raw bespoke implementation
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
            if wire_descriptor(op).is_some() {
                return Some(errors::discriminator_fn_named(&name, &ordered));
            }
            match impl_binding(bound, &op.id) {
                Some(b) if b.raw => Some(errors::outcome_discriminator_fn_named(&name, &ordered)),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_string_literal_escapes_quotes_backslashes_and_control_characters() {
        assert_eq!(rust_string_literal("plain"), "\"plain\"");
        assert_eq!(rust_string_literal("a\"b"), "\"a\\\"b\"");
        assert_eq!(rust_string_literal("a\\b"), "\"a\\\\b\"");
        assert_eq!(rust_string_literal("a\nb"), "\"a\\nb\"");
        assert_eq!(rust_string_literal("a\rb"), "\"a\\rb\"");
        assert_eq!(rust_string_literal("a\tb"), "\"a\\tb\"");
        assert_eq!(rust_string_literal("a\u{1}b"), "\"a\\u{1}b\"");
    }
}

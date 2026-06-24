//! The Go union codec: Go has no sum type, so an internally-tagged union becomes
//! a struct with one pointer per variant plus hand-written `MarshalJSON` /
//! `UnmarshalJSON`. Marshalling flattens the active variant's payload next to the
//! discriminator (via the shared `marshalTagged` helper); unmarshalling peeks the
//! discriminator and decodes into the matching variant.
//!
//! These are emitted as `Decl::Raw` items so the engine renders their text
//! untouched while still collecting their imports — the payload types plus the
//! `encoding/json` and `fmt` standard packages the methods use.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::symbols::symbol_of;
use crate::codegen::tree::{Decl, Raw};
use crate::ir::Member;

/// The standard-library symbols a union's JSON methods reference, declared so the
/// engine collects their imports.
fn stdlib_refs() -> Vec<Symbol> {
    vec![
        Symbol::imported("json", "encoding/json", "json"),
        Symbol::imported("fmt", "fmt", "fmt"),
    ]
}

/// The wire tag for a union member: its `@wire` override, else its name.
fn wire_tag(member: &Member) -> String {
    member
        .traits
        .iter()
        .find(|t| t.id == "core#wire")
        .and_then(|t| t.value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| member.name.clone())
}

/// Build the union item: the variant-pointer struct plus `MarshalJSON` and
/// `UnmarshalJSON`. Each variant identifier is PascalCase; its wire tag rides the
/// discriminator. Payload types and the `encoding/json`/`fmt` packages are
/// declared as refs so their imports are collected.
pub(crate) fn union_item(discriminator: &str, members: &[Member], name: &str) -> Decl {
    let config = CasingConfig::new(CaseStyle::Pascal);
    let variants: Vec<(String, String, String)> = members
        .iter()
        .map(|m| {
            (
                transform(
                    &m.name,
                    crate::codegen::symbol::SymbolKind::Variant,
                    &config,
                    None,
                ),
                wire_tag(m),
                symbol_of(&m.target).name,
            )
        })
        .collect();

    let mut text = format!("type {name} struct {{\n");
    for (ident, _, payload) in &variants {
        text.push_str(&format!("\t{ident} *{payload}\n"));
    }
    text.push_str("}\n\n");

    // MarshalJSON: flatten the active variant next to the discriminator.
    text.push_str(&format!(
        "func (m {name}) MarshalJSON() ([]byte, error) {{\n"
    ));
    text.push_str("\tswitch {\n");
    for (ident, tag, _) in &variants {
        text.push_str(&format!("\tcase m.{ident} != nil:\n"));
        text.push_str(&format!(
            "\t\treturn marshalTagged(\"{discriminator}\", \"{tag}\", m.{ident})\n"
        ));
    }
    text.push_str(&format!(
        "\t}}\n\treturn nil, fmt.Errorf(\"{name}: no variant set\")\n}}\n\n"
    ));

    // UnmarshalJSON: peek the discriminator, decode into the matching variant.
    text.push_str(&format!(
        "func (m *{name}) UnmarshalJSON(data []byte) error {{\n"
    ));
    text.push_str(&format!(
        "\tvar head struct {{\n\t\tTag string `json:\"{discriminator}\"`\n\t}}\n"
    ));
    text.push_str("\tif err := json.Unmarshal(data, &head); err != nil {\n\t\treturn err\n\t}\n");
    text.push_str("\tswitch head.Tag {\n");
    for (ident, tag, payload) in &variants {
        text.push_str(&format!("\tcase \"{tag}\":\n"));
        text.push_str(&format!("\t\tm.{ident} = new({payload})\n"));
        text.push_str(&format!("\t\treturn json.Unmarshal(data, m.{ident})\n"));
    }
    text.push_str(&format!(
        "\t}}\n\treturn fmt.Errorf(\"{name}: unknown variant %q\", head.Tag)\n}}"
    ));

    let mut refs = stdlib_refs();
    refs.extend(members.iter().map(|m| symbol_of(&m.target)));
    Decl::Raw(Raw { text, refs })
}

/// The generic `Entry[K, V]` helper for `@entries` maps: an ordered pair that
/// marshals to and unmarshals from a two-element JSON array `[k, v]`. The
/// assembler emits it once for a module that has any `@entries` field. It
/// references `encoding/json`.
pub(crate) fn entry_helper() -> Decl {
    let text = "\
type Entry[K any, V any] struct {
\tKey   K
\tValue V
}

func (e Entry[K, V]) MarshalJSON() ([]byte, error) {
\treturn json.Marshal([]any{e.Key, e.Value})
}

func (e *Entry[K, V]) UnmarshalJSON(data []byte) error {
\tvar pair [2]json.RawMessage
\tif err := json.Unmarshal(data, &pair); err != nil {
\t\treturn err
\t}
\tif err := json.Unmarshal(pair[0], &e.Key); err != nil {
\t\treturn err
\t}
\treturn json.Unmarshal(pair[1], &e.Value)
}"
    .to_string();
    Decl::Raw(Raw {
        text,
        refs: vec![Symbol::imported("json", "encoding/json", "json")],
    })
}

/// The shared `marshalTagged` helper: flattens a payload's fields next to a
/// discriminator key. The assembler emits it once for a module that has any
/// union. It references `encoding/json`.
pub(crate) fn marshal_tagged_helper() -> Decl {
    let text = "\
func marshalTagged(disc string, tag string, payload any) ([]byte, error) {
\traw, err := json.Marshal(payload)
\tif err != nil {
\t\treturn nil, err
\t}
\tvar fields map[string]json.RawMessage
\tif err := json.Unmarshal(raw, &fields); err != nil {
\t\treturn nil, err
\t}
\ttagJSON, _ := json.Marshal(tag)
\tfields[disc] = tagJSON
\treturn json.Marshal(fields)
}"
    .to_string();
    Decl::Raw(Raw {
        text,
        refs: vec![Symbol::imported("json", "encoding/json", "json")],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::wire_member;

    #[test]
    fn a_union_emits_a_pointer_struct_with_json_methods_and_refs() {
        let members = vec![
            wire_member("card", "models#CardData", Some("CARD")),
            wire_member("bank", "models#BankData", None),
        ];
        let decl = union_item("type", &members, "Method");
        assert!(matches!(&decl, Decl::Raw(raw) if
            // The variant-pointer struct.
            raw.text.contains("type Method struct {")
                && raw.text.contains("\tCard *CardData")
                && raw.text.contains("\tBank *BankData")
            // MarshalJSON dispatches on the set pointer to the wire tag.
                && raw.text.contains("case m.Card != nil:")
                && raw.text.contains("marshalTagged(\"type\", \"CARD\", m.Card)")
                && raw.text.contains("marshalTagged(\"type\", \"bank\", m.Bank)")
            // UnmarshalJSON peeks the discriminator and decodes the variant.
                && raw.text.contains("json:\"type\"")
                && raw.text.contains("case \"CARD\":")
                && raw.text.contains("m.Card = new(CardData)")
                && raw.text.contains("unknown variant %q")
            // Stdlib packages plus both payloads are declared as refs.
                && raw.refs.iter().any(|s| s.name == "json")
                && raw.refs.iter().any(|s| s.name == "fmt")
                && raw.refs.iter().any(|s| s.name == "CardData")
                && raw.refs.iter().any(|s| s.name == "BankData")));
    }

    #[test]
    fn the_entry_helper_is_a_generic_pair_marshalling_to_an_array() {
        let decl = entry_helper();
        assert!(matches!(&decl, Decl::Raw(raw) if
            raw.text.contains("type Entry[K any, V any] struct {")
                && raw.text.contains("func (e Entry[K, V]) MarshalJSON()")
                && raw.text.contains("json.Marshal([]any{e.Key, e.Value})")
                && raw.text.contains("func (e *Entry[K, V]) UnmarshalJSON(")
                && raw.refs.len() == 1
                && raw.refs[0].name == "json"));
    }
}

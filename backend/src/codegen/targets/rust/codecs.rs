//! The Rust codecs: the few constructs serde cannot express through derives and
//! attributes alone.
//!
//! - 64-bit integers travel as JSON strings (a `number` would lose precision
//!   past 2^53 in some consumers), so an `i64`/`u64` field gets a `#[serde(with)]`
//!   module; `bytes` travels as base64 the same way.
//! - The open enum carries a catch-all `Unknown(String)` arm, which serde derive
//!   cannot model, so it is emitted as a hand-written `Serialize`/`Deserialize`.
//! - The internally-tagged union is a `#[serde(tag = ...)]` enum, which serde
//!   *can* derive, but render rules only know the struct shape, so it is emitted
//!   verbatim here with its payload symbols declared as refs for import
//!   collection.
//!
//! All of these are returned as `Decl::Raw` items (or `#[serde(with)]` paths) so
//! the engine renders their text untouched while still collecting their imports.

use crate::codegen::casing::{CaseStyle, CasingConfig};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::rust::render::type_string;
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::targets::rust::types::variant_ident;
use crate::codegen::tree::{Decl, Field, Raw, TypeExpr};
use crate::ir::Member;

/// The `#[serde(with = "...")]` module path a field needs for its wire encoding,
/// or `None` when serde's native handling is correct. 64-bit integers and bytes
/// are the only fields that need a custom codec; an optional one routes through
/// the module's `option` submodule.
pub(crate) fn serde_with(field: &Field) -> Option<String> {
    let base = match &field.ty {
        TypeExpr::Ref(symbol) => match symbol.name.as_str() {
            "i64" => "i64_string",
            "u64" => "u64_string",
            "Vec<u8>" => "base64_bytes",
            _ => return None,
        },
        _ => return None,
    };
    Some(if field.nullable {
        format!("{base}::option")
    } else {
        base.to_string()
    })
}

/// The casing for an open-enum variant and a union variant: a PascalCase Rust
/// identifier derived from the wire value / member name.
fn variant_casing() -> CasingConfig {
    CasingConfig::new(CaseStyle::Pascal)
}

/// Build the open-enum item: the data enum plus its hand-written `as_wire`,
/// `Serialize`, and `Deserialize`. The known wire values map to PascalCase
/// variants; any other string decodes into the catch-all `Unknown(String)`. The
/// item references no imported symbols.
pub(crate) fn enum_item(values: &[(String, Option<i64>)], name: &str) -> Decl {
    let config = variant_casing();
    let variants: Vec<(String, String)> = values
        .iter()
        .map(|(wire, _)| (variant_ident(wire, &config), wire.clone()))
        .collect();

    let mut text = String::new();
    text.push_str("#[derive(Clone, Debug)]\n");
    text.push_str(&format!("pub enum {name} {{\n"));
    for (ident, _) in &variants {
        text.push_str(&format!("    {ident},\n"));
    }
    text.push_str("    Unknown(String),\n}\n\n");

    text.push_str(&format!("impl {name} {{\n"));
    text.push_str("    fn as_wire(&self) -> &str {\n        match self {\n");
    for (ident, wire) in &variants {
        text.push_str(&format!("            {name}::{ident} => \"{wire}\",\n"));
    }
    text.push_str(&format!(
        "            {name}::Unknown(s) => s.as_str(),\n        }}\n    }}\n}}\n\n"
    ));

    text.push_str(&format!("impl serde::Serialize for {name} {{\n"));
    text.push_str(
        "    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {\n",
    );
    text.push_str("        s.serialize_str(self.as_wire())\n    }\n}\n\n");

    text.push_str(&format!(
        "impl<'de> serde::Deserialize<'de> for {name} {{\n"
    ));
    text.push_str(
        "    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {\n",
    );
    text.push_str("        let s = <String as serde::Deserialize>::deserialize(d)?;\n");
    text.push_str("        Ok(match s.as_str() {\n");
    for (ident, wire) in &variants {
        text.push_str(&format!("            \"{wire}\" => {name}::{ident},\n"));
    }
    text.push_str(&format!(
        "            _ => {name}::Unknown(s),\n        }})\n    }}\n}}"
    ));

    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
    })
}

/// Build the internally-tagged union item: a `#[serde(tag = ...)]` enum whose
/// variants each carry one payload. The variant identifier is PascalCase; its
/// wire tag (the member's `@wire` override, else its name) rides `#[serde(rename)]`.
/// Each payload type is declared as a ref so cross-module payloads are imported.
pub(crate) fn union_item(discriminator: &str, members: &[Member], name: &str) -> Decl {
    let config = variant_casing();

    let mut text = String::new();
    text.push_str("#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n");
    text.push_str(&format!("#[serde(tag = \"{discriminator}\")]\n"));
    text.push_str(&format!("pub enum {name} {{\n"));
    for member in members {
        let ident = variant_ident(&member.name, &config);
        let tag = wire_tag(member);
        let payload = type_string(&TypeExpr::Ref(symbol_of(&member.target)));
        if tag != ident {
            text.push_str(&format!("    #[serde(rename = \"{tag}\")]\n"));
        }
        text.push_str(&format!("    {ident}({payload}),\n"));
    }
    text.push('}');

    let refs: Vec<Symbol> = members.iter().map(|m| symbol_of(&m.target)).collect();
    Decl::Raw(Raw { text, refs })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::ir::{Prim, Trait, Tref};
    use serde_json::json;

    fn values(pairs: Vec<&str>) -> Vec<(String, Option<i64>)> {
        pairs.into_iter().map(|v| (v.to_string(), None)).collect()
    }

    fn member(name: &str, payload_id: &str, wire: Option<&str>) -> Member {
        Member {
            name: name.into(),
            target: Tref::Ref {
                id: payload_id.into(),
                args: vec![],
            },
            required: true,
            default: None,
            constraints: vec![],
            traits: wire
                .map(|w| {
                    vec![Trait {
                        id: "core#wire".into(),
                        value: json!(w),
                    }]
                })
                .unwrap_or_default(),
        }
    }

    fn field(name: &str, ty: TypeExpr, nullable: bool) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty,
            nullable,
            wire: None,
        }
    }

    #[test]
    fn serde_with_targets_only_the_wide_integers_and_bytes() {
        assert_eq!(
            serde_with(&field("a", TypeExpr::Ref(Symbol::builtin("i64")), false)).as_deref(),
            Some("i64_string")
        );
        assert_eq!(
            serde_with(&field("a", TypeExpr::Ref(Symbol::builtin("u64")), false)).as_deref(),
            Some("u64_string")
        );
        assert_eq!(
            serde_with(&field(
                "a",
                TypeExpr::Ref(Symbol::builtin("Vec<u8>")),
                false
            ))
            .as_deref(),
            Some("base64_bytes")
        );
        // A nullable one routes through the option submodule.
        assert_eq!(
            serde_with(&field("a", TypeExpr::Ref(Symbol::builtin("i64")), true)).as_deref(),
            Some("i64_string::option")
        );
        // Narrow integers and other types need no custom codec.
        assert_eq!(
            serde_with(&field("a", TypeExpr::Ref(Symbol::builtin("i32")), false)),
            None
        );
        assert_eq!(
            serde_with(&field(
                "a",
                TypeExpr::list(TypeExpr::Ref(Symbol::builtin("i64"))),
                false
            )),
            None
        );
    }

    #[test]
    fn an_open_enum_emits_a_data_enum_with_an_unknown_arm_and_custom_impls() {
        let decl = enum_item(&values(vec!["pending", "card_present"]), "Status");
        // Wire values map to PascalCase variants; the catch-all is Unknown(String);
        // the custom impls carry the wire strings verbatim.
        assert!(matches!(&decl, Decl::Raw(raw) if
            raw.refs.is_empty()
                && raw.text.contains("pub enum Status {")
                && raw.text.contains("    Pending,")
                && raw.text.contains("    CardPresent,")
                && raw.text.contains("    Unknown(String),")
                && raw.text.contains("Status::Pending => \"pending\",")
                && raw.text.contains("\"card_present\" => Status::CardPresent,")
                && raw.text.contains("_ => Status::Unknown(s),")
                && raw.text.contains("impl serde::Serialize for Status")
                && raw.text.contains("impl<'de> serde::Deserialize<'de> for Status")));
    }

    #[test]
    fn an_empty_enum_is_just_the_unknown_arm() {
        let decl = enum_item(&[], "Empty");
        assert!(matches!(&decl, Decl::Raw(raw) if
            raw.text.contains("    Unknown(String),")
                && raw.text.contains("Empty::Unknown(s) => s.as_str(),")));
    }

    #[test]
    fn a_union_emits_a_tagged_enum_and_declares_payload_refs() {
        let members = vec![
            member("card", "cards#CardData", Some("CARD")),
            member("bank", "billing#BankData", None),
            // A wire override that already equals the PascalCase identifier needs
            // no rename, exercising the no-rename path.
            member("wire", "billing#WireData", Some("Wire")),
        ];
        let decl = union_item("type", &members, "Method");
        assert!(matches!(&decl, Decl::Raw(raw) if
            raw.text.contains("#[serde(tag = \"type\")]")
                && raw.text.contains("pub enum Method {")
                // The @wire override is the tag; the identifier stays PascalCase,
                // so a rename carries the wire value.
                && raw.text.contains("    #[serde(rename = \"CARD\")]")
                && raw.text.contains("    Card(CardData),")
                // No override: the lowercase member name is the tag, which still
                // differs from the PascalCase identifier, so a rename is emitted.
                && raw.text.contains("    #[serde(rename = \"bank\")]")
                && raw.text.contains("    Bank(BankData),")
                // Override equals the identifier: no rename line for this variant.
                && raw.text.contains("    Wire(WireData),")
                && !raw.text.contains("rename = \"Wire\"")
                // Payload symbols are declared so cross-module ones get imported.
                && raw.refs.len() == 3
                && raw.refs.iter().any(|s| s.name == "CardData")));
    }

    #[test]
    fn the_prim_bytes_symbol_name_matches_the_codec_trigger() {
        // serde_with keys on the symbol name the symbol table produces for bytes.
        assert_eq!(symbol_of(&Tref::Prim(Prim::Bytes)).name, "Vec<u8>");
    }
}

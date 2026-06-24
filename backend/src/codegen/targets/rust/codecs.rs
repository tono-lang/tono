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

/// The branded well-known newtypes. They are `#[serde(transparent)]` wrappers
/// over `String`, so they serialize exactly as their inner value while staying
/// distinct types in code. The assembler prepends these to a module.
pub(crate) fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration"]
        .iter()
        .map(|name| {
            Decl::Raw(Raw {
                text: format!(
                    "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n\
                     #[serde(transparent)]\n\
                     pub struct {name}(pub String);"
                ),
                refs: Vec::new(),
            })
        })
        .collect()
}

/// The hand-written `#[serde(with)]` helper modules: a 64-bit integer travels as
/// a JSON string and `bytes` as base64, each with an `option` submodule for the
/// nullable field path. The assembler prepends these to a module.
pub(crate) fn runtime_helpers() -> Vec<Decl> {
    [
        int_string_module("i64"),
        int_string_module("u64"),
        BASE64_BYTES_MODULE.to_string(),
    ]
    .into_iter()
    .map(|text| {
        Decl::Raw(Raw {
            text,
            refs: Vec::new(),
        })
    })
    .collect()
}

const INDENT: &str = "    ";

/// The `{ty}_string` module: a 64-bit integer that travels as a JSON string.
fn int_string_module(ty: &str) -> String {
    format!(
        "pub mod {ty}_string {{\n\
         {INDENT}pub fn serialize<S: serde::Serializer>(v: &{ty}, s: S) -> Result<S::Ok, S::Error> {{\n\
         {INDENT}{INDENT}s.serialize_str(&v.to_string())\n\
         {INDENT}}}\n\
         {INDENT}pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<{ty}, D::Error> {{\n\
         {INDENT}{INDENT}let s = <String as serde::Deserialize>::deserialize(d)?;\n\
         {INDENT}{INDENT}s.parse().map_err(serde::de::Error::custom)\n\
         {INDENT}}}\n\
         {INDENT}pub mod option {{\n\
         {INDENT}{INDENT}pub fn serialize<S: serde::Serializer>(v: &Option<{ty}>, s: S) -> Result<S::Ok, S::Error> {{\n\
         {INDENT}{INDENT}{INDENT}match v {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}Some(n) => s.serialize_str(&n.to_string()),\n\
         {INDENT}{INDENT}{INDENT}{INDENT}None => s.serialize_none(),\n\
         {INDENT}{INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<{ty}>, D::Error> {{\n\
         {INDENT}{INDENT}{INDENT}let o = <Option<String> as serde::Deserialize>::deserialize(d)?;\n\
         {INDENT}{INDENT}{INDENT}match o {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),\n\
         {INDENT}{INDENT}{INDENT}{INDENT}None => Ok(None),\n\
         {INDENT}{INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}}}\n\
         }}"
    )
}

/// The base64 helper module: `bytes` travels as a base64 JSON string. The
/// encoder/decoder are hand-rolled (no external crate), standard alphabet with
/// padding.
const BASE64_BYTES_MODULE: &str = r#"pub mod base64_bytes {
    const ALPHABET: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(bytes: &[u8]) -> String {
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            out.push(ALPHABET[((n >> 18) & 63) as usize] as char);
            out.push(ALPHABET[((n >> 12) & 63) as usize] as char);
            out.push(if chunk.len() > 1 { ALPHABET[((n >> 6) & 63) as usize] as char } else { '=' });
            out.push(if chunk.len() > 2 { ALPHABET[(n & 63) as usize] as char } else { '=' });
        }
        out
    }

    pub fn decode(s: &str) -> Result<Vec<u8>, String> {
        fn val(c: u8) -> Result<u32, String> {
            match c {
                b'A'..=b'Z' => Ok(u32::from(c - b'A')),
                b'a'..=b'z' => Ok(u32::from(c - b'a' + 26)),
                b'0'..=b'9' => Ok(u32::from(c - b'0' + 52)),
                b'+' => Ok(62),
                b'/' => Ok(63),
                _ => Err("invalid base64".to_string()),
            }
        }
        let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
        let mut out = Vec::new();
        for chunk in bytes.chunks(4) {
            let mut n = 0u32;
            for (i, &c) in chunk.iter().enumerate() {
                n |= val(c)? << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if chunk.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if chunk.len() > 3 {
                out.push(n as u8);
            }
        }
        Ok(out)
    }

    pub fn serialize<S: serde::Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&encode(v))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = <String as serde::Deserialize>::deserialize(d)?;
        decode(&s).map_err(serde::de::Error::custom)
    }
    pub mod option {
        pub fn serialize<S: serde::Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(b) => s.serialize_str(&super::encode(b)),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            let o = <Option<String> as serde::Deserialize>::deserialize(d)?;
            match o {
                Some(s) => super::decode(&s).map(Some).map_err(serde::de::Error::custom),
                None => Ok(None),
            }
        }
    }
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::symbol::Symbol;
    use crate::codegen::test_support::wire_member;
    use crate::ir::{Prim, Tref};

    fn values(pairs: Vec<&str>) -> Vec<(String, Option<i64>)> {
        pairs.into_iter().map(|v| (v.to_string(), None)).collect()
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
            wire_member("card", "cards#card_data", Some("CARD")),
            wire_member("bank", "billing#bank_data", None),
            // A wire override that already equals the PascalCase identifier needs
            // no rename, exercising the no-rename path.
            wire_member("wire", "billing#wire_data", Some("Wire")),
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

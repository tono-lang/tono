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
use crate::codegen::conventions::doc_of;
use crate::codegen::doc;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::rust::render::{deprecated_attr, type_string};
use crate::codegen::targets::rust::symbols::symbol_of;
use crate::codegen::targets::rust::types::variant_ident;
use crate::codegen::tree::{Decl, Field, Raw, TypeExpr};
use crate::ir::{EnumBacking, EnumValue, Member};

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

/// Build the open-enum *type definition*: just the data enum with its catch-all
/// `Unknown` arm, which belongs in the types file. Its hand-written serde impls are
/// emitted separately by [`enum_serde_item`] into the serde file. The catch-all
/// carries the backing scalar — a `String` for a string-backed enum, an `i64` for
/// an int-backed one — so an unknown wire value survives a round-trip. The item
/// references no imported symbols.
pub(crate) fn enum_item(
    backing: &EnumBacking,
    values: &[EnumValue],
    name: &str,
    deprecated: Option<&str>,
    doc: Option<&str>,
) -> Decl {
    let config = variant_casing();
    let unknown = match backing {
        EnumBacking::String => "    Unknown(String),\n}",
        EnumBacking::Int => "    Unknown(i64),\n}",
    };
    let mut text = String::new();
    if let Some(d) = doc {
        text.push_str(&doc::rustdoc(d, ""));
    }
    let attr = deprecated_attr(deprecated);
    if !attr.is_empty() {
        text.push_str(&attr);
        text.push('\n');
    }
    text.push_str("#[derive(Clone, Debug)]\n");
    text.push_str(&format!("pub enum {name} {{\n"));
    for v in values {
        if let Some(d) = doc_of(&v.traits) {
            text.push_str(&doc::rustdoc(&d, "    "));
        }
        text.push_str(&format!("    {},\n", variant_ident(&v.name, &config)));
    }
    text.push_str(unknown);

    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
        ..Raw::default()
    })
}

/// Build the open-enum *serde item*: a single `open_enum!` invocation that expands
/// to the hand-written `Serialize`/`Deserialize`, which belong in the serde file.
/// The macro is defined once per file by [`open_enum_macro`]; each enum reduces to
/// its name, its backing wire type, and the known (variant, wire-literal) pairs, so
/// the string and int paths share one emitter and the boilerplate is written once.
/// A string-backed enum travels as a JSON string and decodes from `String`; an
/// int-backed one as a JSON integer and decodes from `i64`. The orphan rule permits
/// the expanded impls in a sibling module because the enum type is local to the
/// crate; the serde file's `use crate::<module>::*` brings it into scope. The item
/// references no imported symbols.
pub(crate) fn enum_serde_item(backing: &EnumBacking, values: &[EnumValue], name: &str) -> Decl {
    let config = variant_casing();
    // The only per-backing difference is data: the decode wire type and how each
    // known value is spelled as a Rust literal (a quoted string vs an `i64`).
    let (wire_ty, arms): (&str, Vec<(String, String)>) = match backing {
        EnumBacking::String => (
            "String",
            values
                .iter()
                .map(|v| (variant_ident(&v.name, &config), format!("\"{}\"", v.name)))
                .collect(),
        ),
        EnumBacking::Int => (
            "i64",
            values
                .iter()
                .map(|v| {
                    (
                        variant_ident(&v.name, &config),
                        format!("{}i64", v.value.unwrap_or(0)),
                    )
                })
                .collect(),
        ),
    };

    let mut text = format!("open_enum!({name}: {wire_ty} {{\n");
    for (ident, repr) in &arms {
        text.push_str(&format!("    {ident} => {repr},\n"));
    }
    text.push_str("});");

    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
        ..Raw::default()
    })
}

/// The `open_enum!` declarative macro, emitted once into any serde file that holds
/// an open enum. It expands a name, a backing wire type, and the known
/// (variant, wire-literal) pairs into the hand-written `Serialize`/`Deserialize`
/// serde derive cannot model — the catch-all `Unknown` arm that lets an
/// unrecognized wire value survive a round-trip. Defining it once and invoking it
/// per enum is what keeps each enum's codec a few non-duplicated lines instead of a
/// repeated impl block. Serializing through the wire value's own `Serialize` picks
/// `serialize_str`/`serialize_i64` by its type, so the macro needs no per-backing
/// branch; decoding compares the deserialized wire value against each known literal.
pub(crate) fn open_enum_macro() -> Decl {
    Decl::Raw(Raw {
        text: OPEN_ENUM_MACRO.to_string(),
        refs: Vec::new(),
        ..Raw::default()
    })
}

const OPEN_ENUM_MACRO: &str = r#"macro_rules! open_enum {
    ($name:ident : $wire:ty { $($variant:ident => $repr:expr),* $(,)? }) => {
        impl serde::Serialize for $name {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                match self {
                    $($name::$variant => serde::Serialize::serialize(&$repr, s),)*
                    $name::Unknown(v) => serde::Serialize::serialize(v, s),
                }
            }
        }
        impl<'de> serde::Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let v = <$wire as serde::Deserialize>::deserialize(d)?;
                $(if v == $repr {
                    return Ok($name::$variant);
                })*
                Ok($name::Unknown(v))
            }
        }
    };
}"#;

/// Build the internally-tagged union item: a `#[serde(tag = ...)]` enum whose
/// variants each carry one payload. The variant identifier is PascalCase; its
/// wire tag (the member's `@wire` override, else its name) rides `#[serde(rename)]`.
/// Each payload type is declared as a ref so cross-module payloads are imported.
pub(crate) fn union_item(
    discriminator: &str,
    members: &[Member],
    name: &str,
    deprecated: Option<&str>,
    doc: Option<&str>,
) -> Decl {
    let config = variant_casing();

    let mut text = String::new();
    if let Some(d) = doc {
        text.push_str(&doc::rustdoc(d, ""));
    }
    let attr = deprecated_attr(deprecated);
    if !attr.is_empty() {
        text.push_str(&attr);
        text.push('\n');
    }
    text.push_str("#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n");
    text.push_str(&format!("#[serde(tag = \"{discriminator}\")]\n"));
    text.push_str(&format!("pub enum {name} {{\n"));
    for member in members {
        let ident = variant_ident(&member.name, &config);
        let tag = wire_tag(member);
        let payload = type_string(&TypeExpr::Ref(symbol_of(&member.target)));
        if let Some(d) = doc_of(&member.traits) {
            text.push_str(&doc::rustdoc(&d, "    "));
        }
        if tag != ident {
            text.push_str(&format!("    #[serde(rename = \"{tag}\")]\n"));
        }
        text.push_str(&format!("    {ident}({payload}),\n"));
    }
    text.push('}');

    let refs: Vec<Symbol> = members.iter().map(|m| symbol_of(&m.target)).collect();
    Decl::Raw(Raw {
        text,
        refs,
        ..Raw::default()
    })
}

/// The wire tag for a union member: its `@wire` override, else its name.
fn wire_tag(member: &Member) -> String {
    crate::codegen::conventions::wire_key(member)
}

/// The branded well-known newtypes. They are `#[serde(transparent)]` wrappers
/// over `String`, so they serialize exactly as their inner value while staying
/// distinct types in code. The assembler prepends these to a module.
pub(crate) fn well_known_decls() -> Vec<Decl> {
    ["Timestamp", "LocalDate", "Duration"]
        .iter()
        .map(|name| {
            Decl::raw_providing(
                name,
                format!(
                    "#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]\n\
                     #[serde(transparent)]\n\
                     pub struct {name}(pub String);"
                ),
                Vec::new(),
            )
        })
        .collect()
}

/// Which `#[serde(with)]` helper modules a module's fields need. Each is emitted
/// into the serde file only when some field routes through it, and the types file
/// imports exactly this set from the serde file (the `with = "..."` paths resolve
/// through that `use`). A module with no wide integer and no bytes needs none.
#[derive(Clone, Copy, Default)]
pub(crate) struct HelperSet {
    pub i64_string: Used,
    pub u64_string: Used,
    pub base64_bytes: Used,
}

/// How a helper module is reached: directly, through its `option` submodule, or
/// both. Tracked apart because the submodule is a separate declaration, and an
/// SDK with no nullable field of that type must not carry it.
#[derive(Clone, Copy, Default)]
pub(crate) struct Used {
    pub plain: bool,
    pub option: bool,
}

impl Used {
    fn any(self) -> bool {
        self.plain || self.option
    }
}

impl HelperSet {
    /// Fold the helper a single field routes through (if any) into the set.
    pub(crate) fn add_field(&mut self, field: &Field) {
        if let TypeExpr::Ref(symbol) = &field.ty {
            let used = match symbol.name.as_str() {
                "i64" => &mut self.i64_string,
                "u64" => &mut self.u64_string,
                "Vec<u8>" => &mut self.base64_bytes,
                _ => return,
            };
            // Which of the two the field routes through is what `serde_with`
            // decides, so the same test decides what gets emitted.
            if field.nullable {
                used.option = true;
            } else {
                used.plain = true;
            }
        }
    }

    /// The helper modules this set contains, grouped by the SDK-root group that
    /// declares them: a wide integer travels as a string, and bytes as base64,
    /// which are two subjects and so two groups.
    pub(crate) fn by_group(self) -> Vec<(&'static str, Vec<&'static str>)> {
        let mut number = Vec::new();
        if self.i64_string.any() {
            number.push("i64_string");
        }
        if self.u64_string.any() {
            number.push("u64_string");
        }
        let mut bytes = Vec::new();
        if self.base64_bytes.any() {
            bytes.push("base64_bytes");
        }
        [("number", number), ("bytes", bytes)]
            .into_iter()
            .filter(|(_, names)| !names.is_empty())
            .collect()
    }
}

/// The hand-written `#[serde(with)]` helper modules the set selects: a 64-bit
/// integer travels as a JSON string and `bytes` as base64, each with an `option`
/// submodule for the nullable field path. Only the ones some field uses are
/// emitted, into the serde file.
pub(crate) fn runtime_helpers(helpers: HelperSet, group: &str) -> Vec<Decl> {
    let mut texts: Vec<String> = Vec::new();
    if group == "number" {
        if helpers.i64_string.any() {
            texts.push(int_string_module("i64", helpers.i64_string.option));
        }
        if helpers.u64_string.any() {
            texts.push(int_string_module("u64", helpers.u64_string.option));
        }
    }
    if group == "bytes" && helpers.base64_bytes.any() {
        texts.push(base64_bytes_module(helpers.base64_bytes.option));
    }
    texts
        .into_iter()
        .map(|text| {
            Decl::Raw(Raw {
                text,
                refs: Vec::new(),
                ..Raw::default()
            })
        })
        .collect()
}

const INDENT: &str = "    ";

/// The `{ty}_string` module: a 64-bit integer that travels as a string only in
/// human-readable formats (JSON), staying native in binary ones. Branching on
/// `is_human_readable` keeps the type format-agnostic — it never hardcodes JSON.
fn int_string_module(ty: &str, nullable: bool) -> String {
    let option = if nullable {
        int_string_option(ty)
    } else {
        String::new()
    };
    format!(
        "pub mod {ty}_string {{\n\
         {INDENT}pub fn serialize<S: serde::Serializer>(v: &{ty}, s: S) -> Result<S::Ok, S::Error> {{\n\
         {INDENT}{INDENT}if s.is_human_readable() {{\n\
         {INDENT}{INDENT}{INDENT}s.serialize_str(&v.to_string())\n\
         {INDENT}{INDENT}}} else {{\n\
         {INDENT}{INDENT}{INDENT}serde::Serialize::serialize(v, s)\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}}}\n\
         {INDENT}pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<{ty}, D::Error> {{\n\
         {INDENT}{INDENT}if d.is_human_readable() {{\n\
         {INDENT}{INDENT}{INDENT}let s = <String as serde::Deserialize>::deserialize(d)?;\n\
         {INDENT}{INDENT}{INDENT}s.parse().map_err(serde::de::Error::custom)\n\
         {INDENT}{INDENT}}} else {{\n\
         {INDENT}{INDENT}{INDENT}<{ty} as serde::Deserialize>::deserialize(d)\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}}}\n\
         {option}}}"
    )
}

/// The `option` submodule of `{ty}_string`, emitted only when some nullable
/// field routes through it: a null stays null, a value goes through the parent.
fn int_string_option(ty: &str) -> String {
    format!(
        "{INDENT}pub mod option {{\n\
         {INDENT}{INDENT}pub fn serialize<S: serde::Serializer>(v: &Option<{ty}>, s: S) -> Result<S::Ok, S::Error> {{\n\
         {INDENT}{INDENT}{INDENT}match v {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}Some(n) => super::serialize(n, s),\n\
         {INDENT}{INDENT}{INDENT}{INDENT}None => s.serialize_none(),\n\
         {INDENT}{INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<{ty}>, D::Error> {{\n\
         {INDENT}{INDENT}{INDENT}if d.is_human_readable() {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}let o = <Option<String> as serde::Deserialize>::deserialize(d)?;\n\
         {INDENT}{INDENT}{INDENT}{INDENT}match o {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}{INDENT}Some(s) => s.parse().map(Some).map_err(serde::de::Error::custom),\n\
         {INDENT}{INDENT}{INDENT}{INDENT}{INDENT}None => Ok(None),\n\
         {INDENT}{INDENT}{INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}{INDENT}}} else {{\n\
         {INDENT}{INDENT}{INDENT}{INDENT}<Option<{ty}> as serde::Deserialize>::deserialize(d)\n\
         {INDENT}{INDENT}{INDENT}}}\n\
         {INDENT}{INDENT}}}\n\
         {INDENT}}}\n"
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
        // Line breaks are not data; every other length and padding rule is
        // checked before any byte is produced, so a malformed value is an error
        // rather than bytes nobody sent.
        let bytes: Vec<u8> = s.bytes().filter(|&c| c != b'\n' && c != b'\r').collect();
        let pad = bytes.iter().rev().take_while(|&&c| c == b'=').count();
        let body = &bytes[..bytes.len() - pad];
        if bytes.len() % 4 != 0 || pad > 2 || body.contains(&b'=') {
            return Err("invalid base64".to_string());
        }
        let mut out = Vec::new();
        for chunk in body.chunks(4) {
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
        if s.is_human_readable() {
            s.serialize_str(&encode(v))
        } else {
            s.serialize_bytes(v)
        }
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        if d.is_human_readable() {
            let s = <String as serde::Deserialize>::deserialize(d)?;
            decode(&s).map_err(serde::de::Error::custom)
        } else {
            <Vec<u8> as serde::Deserialize>::deserialize(d)
        }
    }
"#;

/// The `option` submodule of `base64_bytes`, emitted only when some nullable
/// bytes field routes through it.
const BASE64_BYTES_OPTION: &str = r#"    pub mod option {
        pub fn serialize<S: serde::Serializer>(v: &Option<Vec<u8>>, s: S) -> Result<S::Ok, S::Error> {
            match v {
                Some(b) => super::serialize(b, s),
                None => s.serialize_none(),
            }
        }
        pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
            if d.is_human_readable() {
                let o = <Option<String> as serde::Deserialize>::deserialize(d)?;
                match o {
                    Some(s) => super::decode(&s).map(Some).map_err(serde::de::Error::custom),
                    None => Ok(None),
                }
            } else {
                <Option<Vec<u8>> as serde::Deserialize>::deserialize(d)
            }
        }
    }
"#;

/// The base64 helper module, with the `option` submodule only when something
/// nullable reaches it.
fn base64_bytes_module(nullable: bool) -> String {
    let option = if nullable { BASE64_BYTES_OPTION } else { "" };
    format!("{BASE64_BYTES_MODULE}{option}}}")
}

#[cfg(test)]
#[path = "codecs_tests.rs"]
mod tests;

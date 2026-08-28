//! The Rust codec emitter's tests, in a file of their own so the module stays
//! inside the source-size ceiling.

use super::*;
use crate::codegen::symbol::Symbol;
use crate::codegen::test_support::wire_member;
use crate::ir::{Prim, Tref};

fn values(pairs: Vec<&str>) -> Vec<EnumValue> {
    pairs
        .into_iter()
        .map(|v| EnumValue {
            name: v.to_string(),
            value: None,
            traits: vec![],
        })
        .collect()
}

fn int_values(pairs: Vec<(&str, i64)>) -> Vec<EnumValue> {
    pairs
        .into_iter()
        .map(|(v, n)| EnumValue {
            name: v.to_string(),
            value: Some(n),
            traits: vec![],
        })
        .collect()
}

fn field(name: &str, ty: TypeExpr, nullable: bool) -> Field {
    Field {
        tag: None,
        name: Symbol::builtin(name),
        ty,
        nullable,
        wire: None,
        deprecated: None,
        doc: None,
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
fn the_open_enum_definition_holds_the_data_enum_and_no_impls() {
    let decl = enum_item(
        &EnumBacking::String,
        &values(vec!["pending", "card_present"]),
        "Status",
        None,
        None,
    );
    // The definition is just the data enum with its catch-all; the serde impls
    // live in a separate item, so they are absent here.
    assert!(matches!(&decl, Decl::Raw(raw) if
        raw.refs.is_empty()
            && raw.text.contains("pub enum Status {")
            && raw.text.contains("    Pending,")
            && raw.text.contains("    CardPresent,")
            && raw.text.contains("    Unknown(String),")
            && !raw.text.contains("impl serde::Serialize for Status")
            && !raw.text.contains("as_wire")));
}

#[test]
fn the_open_enum_serde_item_is_a_string_backed_macro_invocation() {
    let decl = enum_serde_item(
        &EnumBacking::String,
        &values(vec!["pending", "card_present"]),
        "Status",
    );
    // The serde item is a single `open_enum!` invocation keyed on the wire
    // strings; the impl boilerplate lives in the macro, emitted once per file.
    assert!(matches!(&decl, Decl::Raw(raw) if
        raw.refs.is_empty()
            && !raw.text.contains("pub enum Status {")
            && raw.text.contains("open_enum!(Status: String {")
            && raw.text.contains("Pending => \"pending\",")
            && raw.text.contains("CardPresent => \"card_present\",")
            && raw.text.contains("});")));
}

#[test]
fn the_open_enum_macro_expands_the_serde_impls_once() {
    let decl = open_enum_macro();
    // The macro carries the hand-written impls serde derive cannot model; each
    // enum reduces to an invocation, so the boilerplate is written one time.
    assert!(matches!(&decl, Decl::Raw(raw) if
        raw.refs.is_empty()
            && raw.text.contains("macro_rules! open_enum {")
            && raw.text.contains("impl serde::Serialize for $name")
            && raw.text.contains("impl<'de> serde::Deserialize<'de> for $name")
            && raw.text.contains("serde::Serialize::serialize(&$repr, s)")
            && raw.text.contains("let v = <$wire as serde::Deserialize>::deserialize(d)?;")
            && raw.text.contains("if v == $repr {")
            && raw.text.contains("Ok($name::Unknown(v))")));
}

#[test]
fn an_int_backed_enum_definition_has_an_i64_unknown_arm() {
    let decl = enum_item(
        &EnumBacking::Int,
        &int_values(vec![("ok", 200), ("not_found", 404)]),
        "HTTPCode",
        None,
        None,
    );
    // The variants are bare; the catch-all carries the backing i64.
    assert!(matches!(&decl, Decl::Raw(raw) if
        raw.text.contains("pub enum HTTPCode {")
            && raw.text.contains("    Ok,")
            && raw.text.contains("    NotFound,")
            && raw.text.contains("    Unknown(i64),")
            && !raw.text.contains("Unknown(String)")));
}

#[test]
fn an_int_backed_enum_serde_item_codes_through_i64_literals() {
    let decl = enum_serde_item(
        &EnumBacking::Int,
        &int_values(vec![("ok", 200), ("not_found", 404)]),
        "HTTPCode",
    );
    // The invocation declares the `i64` wire type and spells each known value as
    // an `i64` literal, so the shared macro decodes and encodes it as an integer.
    assert!(matches!(&decl, Decl::Raw(raw) if
        raw.refs.is_empty()
            && raw.text.contains("open_enum!(HTTPCode: i64 {")
            && raw.text.contains("Ok => 200i64,")
            && raw.text.contains("NotFound => 404i64,")
            && raw.text.contains("});")));
}

#[test]
fn an_empty_enum_definition_is_just_the_unknown_arm() {
    let def = enum_item(&EnumBacking::String, &[], "Empty", None, None);
    assert!(matches!(&def, Decl::Raw(raw) if
        raw.text.contains("    Unknown(String),")
            && raw.text.contains("pub enum Empty {")));
    // The serde item is still a well-formed invocation with no known arms; the
    // macro's `Unknown` fallback handles every value.
    let serde = enum_serde_item(&EnumBacking::String, &[], "Empty");
    assert!(matches!(&serde, Decl::Raw(raw) if
        raw.text.contains("open_enum!(Empty: String {")
            && raw.text.contains("});")));
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
    let decl = union_item("type", &members, "Method", None, None);
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
fn a_deprecated_enum_and_union_lead_with_the_attribute() {
    // The enum and union are Raw text, so the attribute is prepended there.
    let e = enum_item(
        &EnumBacking::String,
        &values(vec!["pending"]),
        "Status",
        Some("use v2"),
        None,
    );
    assert!(matches!(&e, Decl::Raw(raw)
        if raw.text.starts_with("#[deprecated(note = \"use v2\")]\n#[derive(")));

    let members = vec![wire_member("card", "cards#card_data", None)];
    let u = union_item("type", &members, "Method", Some(""), None);
    assert!(matches!(&u, Decl::Raw(raw)
        if raw.text.starts_with("#[deprecated]\n#[derive(")));
}

#[test]
fn the_prim_bytes_symbol_name_matches_the_codec_trigger() {
    // serde_with keys on the symbol name the symbol table produces for bytes.
    assert_eq!(symbol_of(&Tref::Prim(Prim::Bytes)).name, "Vec<u8>");
}

#[test]
fn helper_set_folds_each_wide_field_and_orders_names() {
    let mut set = HelperSet::default();
    set.add_field(&field("a", TypeExpr::Ref(Symbol::builtin("i64")), false));
    set.add_field(&field("b", TypeExpr::Ref(Symbol::builtin("u64")), false));
    set.add_field(&field(
        "c",
        TypeExpr::Ref(Symbol::builtin("Vec<u8>")),
        false,
    ));
    // A narrow integer folds into nothing.
    set.add_field(&field("d", TypeExpr::Ref(Symbol::builtin("i32")), false));
    assert!(set.i64_string.plain && set.u64_string.plain && set.base64_bytes.plain);
    // Nothing nullable reached them, so no `option` submodule is wanted.
    assert!(!set.i64_string.option && !set.u64_string.option && !set.base64_bytes.option);
    // Grouped by subject, in a fixed order so the types file's imports are
    // byte-stable.
    assert_eq!(
        set.by_group(),
        vec![
            ("number", vec!["i64_string", "u64_string"]),
            ("bytes", vec!["base64_bytes"]),
        ]
    );
}

#[test]
fn the_option_submodule_is_emitted_only_for_a_nullable_field() {
    fn text_of(decls: Vec<Decl>) -> String {
        decls
            .iter()
            .filter_map(|d| d.opaque_text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    // It is a declaration of its own, and `serde_with` routes a field to it
    // only when the field is nullable, so the same test decides both.
    let mut plain = HelperSet::default();
    plain.add_field(&field("a", TypeExpr::Ref(Symbol::builtin("i64")), false));
    let out = text_of(runtime_helpers(plain, "number"));
    assert!(out.contains("pub mod i64_string {"));
    assert!(!out.contains("pub mod option {"));

    let mut nullable = HelperSet::default();
    nullable.add_field(&field("a", TypeExpr::Ref(Symbol::builtin("i64")), true));
    let out = text_of(runtime_helpers(nullable, "number"));
    assert!(out.contains("pub mod option {"));
    assert!(out.contains("Option<i64>"));

    let mut bytes = HelperSet::default();
    bytes.add_field(&field("a", TypeExpr::Ref(Symbol::builtin("Vec<u8>")), true));
    let out = text_of(runtime_helpers(bytes, "bytes"));
    assert!(out.contains("pub mod option {"));
    assert!(out.contains("Option<Vec<u8>>"));
}

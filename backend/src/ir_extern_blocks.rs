//! Rust mirror of the language blocks a struct carries and the two foreign
//! shapes built from them: a foreign form (fields the target reads) and an
//! opaque handle (a thing the target calls), both declared inside an `ext`
//! block, plus the blocks a top-level struct carries as its `foreign` trait
//! (an error struct's recognition, a wire struct's Go tags). The frontend's
//! `ir_json_extern.ml` codec is the source of truth. Split out of
//! `ir_extern_model.rs` to stay within the file-size gate.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::ir::{ExternDecl, Shape, Tref};

/// One language's block on a struct. `name` is positional and names the
/// foreign thing: a foreign form's type, an opaque handle's whole storage
/// type, an error struct's sentinel or error type. It is `None` on a wire
/// struct's block, which names nothing foreign: there `fields` is the
/// target's own per-field declaration (a Go struct tag, verbatim). On the
/// other blocks `fields` pairs a tono field with its foreign spelling: the
/// field's foreign type on a form, where the field comes from on an error
/// value. A top-level struct (error or wire) carries its blocks as the
/// `foreign` trait of its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignLang {
    pub lang: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub fields: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignField {
    pub name: String,
    pub r#type: Tref,
}

/// A foreign shape declared inside an `ext` block, mirroring the foreign
/// side's field names/casing verbatim; never a top-level shape, never role-
/// classified, never crosses the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignStruct {
    pub name: String,
    #[serde(default)]
    pub fields: Vec<ForeignField>,
    #[serde(default)]
    pub langs: Vec<ForeignLang>,
}

impl ForeignStruct {
    /// The form's block for `lang`: the one naming the foreign type. A
    /// headless block names no type to hold the form as, so it is no block
    /// of a form (the frontend refuses one there; a raw IR is read the same
    /// way).
    pub fn lang(&self, lang: &str) -> Option<&ForeignLang> {
        self.langs
            .iter()
            .find(|l| l.lang == lang && l.name.is_some())
    }
}

/// An opaque foreign handle whose only members are ext op methods; never
/// serializes, never crosses the wire. Each language block spells the whole
/// storage type verbatim (`Calculator[float64]`, `Box<dyn Calculator<f64>>`);
/// a language with no block does not hold the handle at all.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpaqueType {
    pub name: String,
    #[serde(default)]
    pub langs: Vec<ForeignLang>,
    #[serde(default)]
    pub methods: Vec<ExternDecl>,
}

impl OpaqueType {
    /// The storage type this handle declares for one language, when it
    /// declares one.
    pub fn storage(&self, lang: &str) -> Option<&str> {
        self.langs
            .iter()
            .find(|l| l.lang == lang)
            .and_then(|l| l.name.as_deref())
    }
}

impl ForeignLang {
    /// The language blocks an error struct carries as the `foreign` trait of
    /// its shape (see `ForeignLang`); empty when it declares none.
    pub fn of_shape(shape: &Shape) -> Vec<ForeignLang> {
        shape
            .traits
            .iter()
            .find(|t| t.id == "foreign")
            .and_then(|t| serde_json::from_value(t.value.clone()).ok())
            .unwrap_or_default()
    }

    /// How `lang` recognizes the error shape `id` of `module`: its block,
    /// when the shape declares one for that language. A recognition names
    /// the sentinel or error type first, so a headless block is none.
    pub fn of_error(module: &crate::ir::Module, id: &str, lang: &str) -> Option<ForeignLang> {
        let shape = module.shapes.iter().find(|s| s.id == id)?;
        Self::of_shape(shape)
            .into_iter()
            .find(|l| l.lang == lang && l.name.is_some())
    }

    /// The head of a block that names something foreign: a form's type, a
    /// handle's storage type, an error's sentinel. Every lookup handing out
    /// such a block (`ForeignStruct::lang`, `OpaqueType::storage`,
    /// `of_error`) skips a headless one, so a reader holding the block
    /// holds its head; a headless block reaches no reader of a head.
    pub fn head(&self) -> &str {
        self.name
            .as_deref()
            .expect("a form, handle or error block names its head; the lookups skip a headless one")
    }

    /// The per-field declarations a wire struct's `lang` block carries (a Go
    /// struct tag per tono field, verbatim): the block with no head. A block
    /// with a head is an error struct's recognition, whose keyed entries are
    /// field sources, never tags. The one definition the emitter and its
    /// generation gate share.
    pub fn struct_tags(shape: &Shape, lang: &str) -> BTreeMap<String, String> {
        Self::of_shape(shape)
            .into_iter()
            .find(|l| l.lang == lang && l.name.is_none())
            .map(|l| l.fields)
            .unwrap_or_default()
    }
}

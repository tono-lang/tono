//! Rust mirror of the FFI `ext`/`extern` library surface (module `ext_libs`):
//! per-language module paths, foreign struct/opaque-handle declarations, and
//! `extern` declarations with a per-language call/yields/returns/errors
//! binding. The frontend's `ir_json_extern.ml` codec is the source of truth.
//! Split out of `ir.rs` to stay within the file-size gate. Surface-and-IR
//! only: no typecheck or codegen consumes this yet.

use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

use crate::ir::{Select, Tref};

fn ensure_only(obj: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unexpected key {k:?}")),
        None => Ok(()),
    }
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One argument to an extern call: the caller's own parameter by name, a
/// field-reference path, a struct-literal mapper, a scalar literal, a list,
/// or a nested call (the last three only arise inside a ctor field's value,
/// e.g. `opts { retries: 3 }`). The ctor case is a two-key object, so this
/// does not match a uniform serde tagging mode; the codec is hand-written,
/// mirroring `Tref`.
#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    Param(String),
    Ref(Vec<String>),
    Ctor(CallCtor),
    Lit(Value),
    List(Vec<CallArg>),
    Call(Box<EntryCall>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallCtor {
    pub name: String,
    pub fields: BTreeMap<String, CallArg>,
}

impl Serialize for CallArg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            CallArg::Param(n) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("param", n)?;
                m.end()
            }
            CallArg::Ref(p) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("field", p)?;
                m.end()
            }
            CallArg::Lit(v) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("lit", v)?;
                m.end()
            }
            CallArg::List(xs) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("list", xs)?;
                m.end()
            }
            CallArg::Call(c) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("call", c)?;
                m.end()
            }
            CallArg::Ctor(c) => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("ctor", &c.name)?;
                m.serialize_entry("fields", &c.fields)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for CallArg {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        call_arg_from_value(&v).map_err(serde::de::Error::custom)
    }
}

const CALL_ARG_KEYS: [&str; 6] = ["param", "field", "lit", "list", "call", "ctor"];

fn call_arg_from_value(v: &Value) -> Result<CallArg, String> {
    let obj = v.as_object().ok_or("expected an object")?;
    let present: Vec<&str> = CALL_ARG_KEYS
        .iter()
        .copied()
        .filter(|k| obj.contains_key(*k))
        .collect();
    match present.as_slice() {
        ["param"] => {
            ensure_only(obj, &["param"])?;
            Ok(CallArg::Param(
                obj["param"]
                    .as_str()
                    .ok_or("expected a string")?
                    .to_string(),
            ))
        }
        ["field"] => {
            ensure_only(obj, &["field"])?;
            let arr = obj["field"].as_array().ok_or("expected an array")?;
            let segs = arr
                .iter()
                .map(|x| {
                    x.as_str()
                        .map(str::to_string)
                        .ok_or_else(|| "expected a string".to_string())
                })
                .collect::<Result<_, _>>()?;
            Ok(CallArg::Ref(segs))
        }
        ["lit"] => {
            ensure_only(obj, &["lit"])?;
            Ok(CallArg::Lit(obj["lit"].clone()))
        }
        ["list"] => {
            ensure_only(obj, &["list"])?;
            let arr = obj["list"].as_array().ok_or("expected an array")?;
            let items = arr
                .iter()
                .map(call_arg_from_value)
                .collect::<Result<_, _>>()?;
            Ok(CallArg::List(items))
        }
        ["call"] => {
            ensure_only(obj, &["call"])?;
            let c: EntryCall =
                serde_json::from_value(obj["call"].clone()).map_err(|e| e.to_string())?;
            Ok(CallArg::Call(Box::new(c)))
        }
        ["ctor"] => {
            ensure_only(obj, &["ctor", "fields"])?;
            let name = obj["ctor"].as_str().ok_or("expected a string")?.to_string();
            let fields_obj = obj["fields"].as_object().ok_or("expected an object")?;
            let mut fields = BTreeMap::new();
            for (k, fv) in fields_obj {
                fields.insert(k.clone(), call_arg_from_value(fv)?);
            }
            Ok(CallArg::Ctor(CallCtor { name, fields }))
        }
        [] => Err("call arg object has no recognized variant key".to_string()),
        _ => Err("call arg object has multiple variant keys".to_string()),
    }
}

/// A field's `= ns.fn(args)` value: a call into an extern declared in the
/// `ext` block named `ns`. Resolving `ns`/`fn` against a declared extern is
/// deferred (out of scope).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryCall {
    pub ns: String,
    #[serde(rename = "fn")]
    pub func: String,
    #[serde(default)]
    pub args: Vec<CallArg>,
}

/// One `yields:` position; `r#type` is absent only for the reserved `error`
/// sentinel (`is_error`), never for an ordinary omitted type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YieldsPos {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<Tref>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
}

/// A `returns:` field value: a bare ref into a `yields:`-bound name, or a
/// match over one -- the same shape a member's own `= match` selection uses.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReturnsValue {
    Field(Vec<String>),
    Select(Select),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReturnsField {
    pub name: String,
    pub value: ReturnsValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReturnsLit {
    pub r#type: Tref,
    #[serde(default)]
    pub fields: Vec<ReturnsField>,
}

/// One `errors: { "sentinel" => TypeName }` entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBinding {
    pub sentinel: String,
    pub r#type: String,
}

/// One per-language block inside an `extern`'s body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternLang {
    pub lang: String,
    pub symbol: String,
    #[serde(default)]
    pub call_args: Vec<CallArg>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub yields: Vec<YieldsPos>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub returns: Option<ReturnsLit>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<ErrorBinding>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub sync: bool,
    /// No error return (Go's own convention makes every call fallible by
    /// default, `(value, error)`; this opts a single-return foreign function
    /// like `uuid.NewString() string` out of that shape). Only Go's emitter
    /// reads it, the same way only Rust's reads `sync`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub infallible: bool,
    /// The call receives the target's own cancellation/deadline context in
    /// its idiomatic position (Go: `ctx context.Context` as the first
    /// parameter). Only meaningful on a foreign handle's own method call
    /// (the frontend rejects it elsewhere); a target with no equivalent
    /// convention ignores it the same way Go ignores `sync`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub ctx: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternParam {
    pub name: String,
    pub r#type: Tref,
}

/// A free function inside an `ext` block, or a method inside a `type`
/// opaque handle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternDecl {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ExternParam>,
    pub r#return: Tref,
    #[serde(default)]
    pub langs: Vec<ExternLang>,
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
}

/// One language's spelling of the foreign type an opaque handle names.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceName {
    pub lang: String,
    pub name: String,
}

/// Which instantiation of a foreign generic type an opaque handle names: the
/// foreign type's own name per language (kept distinct from the handle's own
/// tono name, since two handles can instantiate the same foreign generic
/// differently; per language because the same logical handle can be an
/// interface in one target and a trait in another, each spelled by its own
/// library), and the tono argument it is monomorphized with. The frontend
/// expands a shared surface name to one entry per declared language, so a
/// reader only ever looks its own language up.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Instance {
    #[serde(default)]
    pub names: Vec<InstanceName>,
    pub arg: Tref,
}

impl Instance {
    /// The foreign name this instantiation declares for one language, when
    /// the surface named one.
    pub fn name_for(&self, lang: &str) -> Option<&str> {
        self.names
            .iter()
            .find(|n| n.lang == lang)
            .map(|n| n.name.as_str())
    }
}

/// An opaque foreign handle whose only members are extern methods; never
/// serializes, never crosses the wire. `interface` declares the foreign type
/// is abstract, not a concrete struct: nothing about a foreign name says
/// which one it is, and the two hold differently per target (a Go interface
/// is held by value, never behind `*`; a Rust trait is held as
/// `Box<dyn Trait>`, boxed where a constructor yields one of the library's
/// concrete types).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpaqueType {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<Instance>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub interface: bool,
    #[serde(default)]
    pub methods: Vec<ExternDecl>,
}

/// One per-language module path declared by an `ext` block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LangPath {
    pub lang: String,
    pub path: String,
}

/// One `ext <name> { ... }` library declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExtLib {
    pub name: String,
    #[serde(default)]
    pub langs: Vec<LangPath>,
    #[serde(default)]
    pub structs: Vec<ForeignStruct>,
    #[serde(default)]
    pub types: Vec<OpaqueType>,
    #[serde(default)]
    pub externs: Vec<ExternDecl>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(arg: &CallArg) {
        let json = serde_json::to_string(arg).unwrap();
        let back: CallArg = serde_json::from_str(&json).unwrap();
        assert_eq!(&back, arg, "round-trip mismatch for {json}");
    }

    #[test]
    fn every_call_arg_variant_roundtrips() {
        roundtrip(&CallArg::Param("service".into()));
        roundtrip(&CallArg::Ref(vec!["cfg".into(), "Host".into()]));
        roundtrip(&CallArg::Lit(serde_json::json!(3)));
        roundtrip(&CallArg::List(vec![
            CallArg::Param("a".into()),
            CallArg::Lit(serde_json::json!("b")),
        ]));
        roundtrip(&CallArg::Call(Box::new(EntryCall {
            ns: "companyauth".into(),
            func: "sign".into(),
            args: vec![CallArg::Ref(vec!["request".into()])],
        })));
        let mut fields = BTreeMap::new();
        fields.insert("region".to_string(), CallArg::Param("region".into()));
        roundtrip(&CallArg::Ctor(CallCtor {
            name: "ts_opts".into(),
            fields,
        }));
    }

    #[test]
    fn call_arg_rejects_a_non_object() {
        assert!(serde_json::from_str::<CallArg>("1").is_err());
    }

    #[test]
    fn call_arg_rejects_zero_recognized_keys() {
        let err = serde_json::from_str::<CallArg>(r#"{"nope":1}"#).unwrap_err();
        assert!(err.to_string().contains("no recognized variant key"));
    }

    #[test]
    fn call_arg_rejects_multiple_recognized_keys() {
        let err = serde_json::from_str::<CallArg>(r#"{"param":"a","lit":1}"#).unwrap_err();
        assert!(err.to_string().contains("multiple variant keys"));
    }

    #[test]
    fn call_arg_rejects_a_stray_sibling_key() {
        let variants_with_a_stray_key = [
            r#"{"param":"a","extra":1}"#,
            r#"{"field":["a"],"extra":1}"#,
            r#"{"lit":1,"extra":1}"#,
            r#"{"list":[],"extra":1}"#,
            r#"{"call":{"ns":"n","fn":"f"},"extra":1}"#,
            r#"{"ctor":"c","fields":{},"extra":1}"#,
        ];
        for json in variants_with_a_stray_key {
            assert!(
                serde_json::from_str::<CallArg>(json).is_err(),
                "expected a stray sibling key to be rejected: {json}"
            );
        }
    }

    #[test]
    fn call_arg_field_rejects_a_non_string_segment() {
        assert!(serde_json::from_str::<CallArg>(r#"{"field":[1]}"#).is_err());
    }

    #[test]
    fn is_false_reports_the_negation() {
        assert!(is_false(&false));
        assert!(!is_false(&true));
    }

    #[test]
    fn yields_pos_error_sentinel_omits_the_type_and_skips_the_flag_when_false() {
        let ordinary = YieldsPos {
            name: "cfg".into(),
            r#type: Some(Tref::Prim(crate::ir::Prim::String)),
            is_error: false,
        };
        let json = serde_json::to_string(&ordinary).unwrap();
        assert!(!json.contains("is_error"));

        let sentinel = YieldsPos {
            name: "e".into(),
            r#type: None,
            is_error: true,
        };
        let json = serde_json::to_string(&sentinel).unwrap();
        assert!(json.contains(r#""is_error":true"#));
        let back: YieldsPos = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sentinel);
    }

    #[test]
    fn extern_lang_sync_omits_the_flag_when_false() {
        let lang = ExternLang {
            lang: "rust".into(),
            symbol: "load".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(!json.contains("sync"));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }

    #[test]
    fn extern_lang_sync_serializes_true_and_round_trips() {
        let lang = ExternLang {
            lang: "rust".into(),
            symbol: "load".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: true,
            infallible: false,
            ctx: false,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(json.contains(r#""sync":true"#));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }

    #[test]
    fn extern_lang_infallible_omits_the_flag_when_false() {
        let lang = ExternLang {
            lang: "go".into(),
            symbol: "NewString".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(!json.contains("infallible"));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }

    #[test]
    fn extern_lang_infallible_serializes_true_and_round_trips() {
        let lang = ExternLang {
            lang: "go".into(),
            symbol: "NewString".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: true,
            ctx: false,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(json.contains(r#""infallible":true"#));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }

    #[test]
    fn extern_lang_ctx_omits_the_flag_when_false() {
        let lang = ExternLang {
            lang: "go".into(),
            symbol: "Get".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: false,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(!json.contains("ctx"));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }

    #[test]
    fn extern_lang_ctx_serializes_true_and_round_trips() {
        let lang = ExternLang {
            lang: "go".into(),
            symbol: "Get".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            errors: vec![],
            sync: false,
            infallible: false,
            ctx: true,
        };
        let json = serde_json::to_string(&lang).unwrap();
        assert!(json.contains(r#""ctx":true"#));
        let back: ExternLang = serde_json::from_str(&json).unwrap();
        assert_eq!(back, lang);
    }
}

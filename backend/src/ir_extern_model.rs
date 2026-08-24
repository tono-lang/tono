//! Rust mirror of the FFI `ext` library surface (module `ext_libs`):
//! per-language module paths, foreign struct/opaque-handle declarations, and
//! ext op declarations with a per-language call/yields/returns binding. The
//! frontend's `ir_json_extern.ml` codec is the source of truth. Split out of
//! `ir.rs` to stay within the file-size gate.
//!
//! A foreign spelling (every `String` below that is not a tono name) is
//! carried verbatim: the emitter writes it as it stands, qualifying only
//! the identifiers that belong to the library with its module.

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
/// a call into a declared extern (the last three only arise inside a ctor
/// field's value, e.g. `opts { retries: 3 }`), or a bare foreign-symbol call
/// nested inside a language block's own `call:` line (e.g.
/// `WithPrecision(precision)` inside `call: "FromFormula"(expr,
/// WithPrecision(precision))` -- legal as a top-level `call:` argument,
/// unlike `Call`). The ctor case is a two-key object, so this does not match
/// a uniform serde tagging mode; the codec is hand-written, mirroring
/// `Tref`.
#[derive(Debug, Clone, PartialEq)]
pub enum CallArg {
    Param(String),
    /// The parameter under a foreign spelling of its own (wire keys
    /// `param` + `as`): what it crosses the boundary as, for the target to
    /// coerce into (`values: #(Vec<f64>)`, `calcs: #(...Calculator[float64])`).
    ParamAs {
        name: String,
        spelling: String,
    },
    Ref(Vec<String>),
    Ctor(CallCtor),
    Lit(Value),
    List(Vec<CallArg>),
    Call(Box<EntryCall>),
    SymbolCall(SymbolCall),
    /// A declared opaque handle passed as a class reference (wire key
    /// `type`): the library takes the class itself and constructs on its
    /// own, so what crosses is the handle's foreign name for the binding's
    /// language, never a value tono built.
    TypeRef(String),
    /// A position that is not a tono value but a declaration of what the
    /// target binds there, with its type (wire key `foreign`): Go's
    /// `#(ctx context.Context)`.
    Foreign(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallCtor {
    pub name: String,
    pub fields: BTreeMap<String, CallArg>,
    /// The literal under a foreign spelling of its own (wire key `as`, the
    /// same annotation a parameter carries): what it crosses the boundary
    /// as, `&Options` for a library that takes the form by pointer. The
    /// form's own type stays what its language block declares; the `&`
    /// belongs to the argument. Absent for a literal passed as the form's
    /// own type.
    pub spelling: Option<String>,
}

/// A bare foreign-symbol call nested inside a `call:` line's own argument
/// list: no declared `extern` to resolve against, just a symbol string and
/// its own argument list, recursing through `CallArg` the same way a
/// language block's own `call:` line does.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolCall {
    pub symbol: String,
    pub args: Vec<CallArg>,
}

impl Serialize for CallArg {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            CallArg::Param(n) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("param", n)?;
                m.end()
            }
            CallArg::ParamAs { name, spelling } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("param", name)?;
                m.serialize_entry("as", spelling)?;
                m.end()
            }
            CallArg::Foreign(sp) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("foreign", sp)?;
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
                let mut m = s.serialize_map(Some(2 + usize::from(c.spelling.is_some())))?;
                m.serialize_entry("ctor", &c.name)?;
                m.serialize_entry("fields", &c.fields)?;
                if let Some(sp) = &c.spelling {
                    m.serialize_entry("as", sp)?;
                }
                m.end()
            }
            CallArg::SymbolCall(sc) => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("symbol", &sc.symbol)?;
                m.serialize_entry("symbol_args", &sc.args)?;
                m.end()
            }
            CallArg::TypeRef(n) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("type", n)?;
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

const CALL_ARG_KEYS: [&str; 9] = [
    "param", "field", "lit", "list", "call", "ctor", "symbol", "type", "foreign",
];

fn call_arg_from_value(v: &Value) -> Result<CallArg, String> {
    let obj = v.as_object().ok_or("expected an object")?;
    let present: Vec<&str> = CALL_ARG_KEYS
        .iter()
        .copied()
        .filter(|k| obj.contains_key(*k))
        .collect();
    match present.as_slice() {
        ["param"] => {
            ensure_only(obj, &["param", "as"])?;
            let name = obj["param"]
                .as_str()
                .ok_or("expected a string")?
                .to_string();
            match obj.get("as") {
                None => Ok(CallArg::Param(name)),
                Some(sp) => Ok(CallArg::ParamAs {
                    name,
                    spelling: sp.as_str().ok_or("expected a string")?.to_string(),
                }),
            }
        }
        ["foreign"] => {
            ensure_only(obj, &["foreign"])?;
            Ok(CallArg::Foreign(
                obj["foreign"]
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
        ["type"] => {
            ensure_only(obj, &["type"])?;
            Ok(CallArg::TypeRef(
                obj["type"].as_str().ok_or("expected a string")?.to_string(),
            ))
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
            ensure_only(obj, &["ctor", "fields", "as"])?;
            let name = obj["ctor"].as_str().ok_or("expected a string")?.to_string();
            let fields_obj = obj["fields"].as_object().ok_or("expected an object")?;
            let mut fields = BTreeMap::new();
            for (k, fv) in fields_obj {
                fields.insert(k.clone(), call_arg_from_value(fv)?);
            }
            let spelling = match obj.get("as") {
                None => None,
                Some(sp) => Some(sp.as_str().ok_or("expected a string")?.to_string()),
            };
            Ok(CallArg::Ctor(CallCtor {
                name,
                fields,
                spelling,
            }))
        }
        ["symbol"] => {
            ensure_only(obj, &["symbol", "symbol_args"])?;
            let symbol = obj["symbol"]
                .as_str()
                .ok_or("expected a string")?
                .to_string();
            let args = match obj.get("symbol_args") {
                None => Vec::new(),
                Some(v) => {
                    let arr = v.as_array().ok_or("expected an array")?;
                    arr.iter()
                        .map(call_arg_from_value)
                        .collect::<Result<_, _>>()?
                }
            };
            Ok(CallArg::SymbolCall(SymbolCall { symbol, args }))
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

/// One `yields:` position. `r#type` is the tono type it carries; it is
/// absent for the reserved `error` sentinel (`is_error`) and for a position
/// declared under a foreign spelling (`foreign`: what the call really
/// returns, for the target to coerce into the declared return or refuse
/// naming both).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct YieldsPos {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub r#type: Option<Tref>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub is_error: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreign: Option<String>,
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

/// One per-language block inside an ext op's body. `symbol` is the
/// callee's whole foreign spelling: a function, a generic instantiation
/// (`FromConstant[float64]`), a class under `new` (`new ConstantCalculator`),
/// a static method on a type (`FormulaCalculator::parse`). Each emitter
/// reads the identifiers out of it to import, and writes the rest as is.
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
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternParam {
    pub name: String,
    pub r#type: Tref,
}

/// A free function inside an `ext` block, or a method of an opaque handle.
/// `r#async` lists the languages where the foreign call itself is
/// asynchronous (absent means synchronous at the boundary); `errors` lists
/// the declared error shapes it can raise, in test order, each recognized
/// through its own `foreign` trait (see `ForeignLang`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternDecl {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ExternParam>,
    pub r#return: Tref,
    #[serde(default)]
    pub langs: Vec<ExternLang>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub r#async: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

impl ExternDecl {
    /// Whether the foreign call is asynchronous in `lang`: the boundary is
    /// synchronous unless the op says otherwise.
    pub fn is_async(&self, lang: &str) -> bool {
        self.r#async.iter().any(|l| l == lang)
    }
}

/// One language's block on a struct. `name` is positional and names the
/// foreign thing: a foreign form's type, an opaque handle's whole storage
/// type, an error struct's sentinel or error type. `fields` pairs a tono
/// field with its foreign spelling: the field's foreign type on a form,
/// where the field comes from on an error value. An error struct carries
/// its blocks as the `foreign` trait of its shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForeignLang {
    pub lang: String,
    pub name: String,
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
    pub fn lang(&self, lang: &str) -> Option<&ForeignLang> {
        self.langs.iter().find(|l| l.lang == lang)
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
            .map(|l| l.name.as_str())
    }
}

impl ForeignLang {
    /// The language blocks an error struct carries as the `foreign` trait of
    /// its shape (see `ForeignLang`); empty when it declares none.
    pub fn of_shape(shape: &crate::ir::Shape) -> Vec<ForeignLang> {
        shape
            .traits
            .iter()
            .find(|t| t.id == "foreign")
            .and_then(|t| serde_json::from_value(t.value.clone()).ok())
            .unwrap_or_default()
    }

    /// How `lang` recognizes the error shape `id` of `module`: its block,
    /// when the shape declares one for that language.
    pub fn of_error(module: &crate::ir::Module, id: &str, lang: &str) -> Option<ForeignLang> {
        let shape = module.shapes.iter().find(|s| s.id == id)?;
        Self::of_shape(shape).into_iter().find(|l| l.lang == lang)
    }
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
            fields: fields.clone(),
            spelling: None,
        }));
        roundtrip(&CallArg::Ctor(CallCtor {
            name: "go_opts".into(),
            fields,
            spelling: Some("&Options".into()),
        }));
        roundtrip(&CallArg::SymbolCall(SymbolCall {
            symbol: "WithPrecision".into(),
            args: vec![CallArg::Param("precision".into())],
        }));
        roundtrip(&CallArg::SymbolCall(SymbolCall {
            symbol: "Bare".into(),
            args: vec![],
        }));
        roundtrip(&CallArg::TypeRef("answer_calculator".into()));
        roundtrip(&CallArg::SymbolCall(SymbolCall {
            symbol: "Pick".into(),
            args: vec![CallArg::TypeRef("answer_calculator".into())],
        }));
        roundtrip(&CallArg::ParamAs {
            name: "values".into(),
            spelling: "Vec<f64>".into(),
        });
        roundtrip(&CallArg::Foreign("ctx context.Context".into()));
    }

    #[test]
    fn a_spelled_parameter_serializes_under_param_and_as() {
        let json = serde_json::to_value(CallArg::ParamAs {
            name: "values".into(),
            spelling: "[]float64".into(),
        })
        .unwrap();
        assert_eq!(
            json,
            serde_json::json!({"param": "values", "as": "[]float64"})
        );
        assert!(serde_json::from_str::<CallArg>(r#"{"param": "a", "as": 1}"#).is_err());
        assert!(serde_json::from_str::<CallArg>(r#"{"foreign": 1}"#).is_err());
    }

    #[test]
    fn a_class_reference_serializes_under_the_type_key() {
        let json = serde_json::to_value(CallArg::TypeRef("answer_calculator".into())).unwrap();
        assert_eq!(json, serde_json::json!({"type": "answer_calculator"}));
        assert!(serde_json::from_str::<CallArg>(r#"{"type": 1}"#).is_err());
        assert!(serde_json::from_str::<CallArg>(r#"{"type": "a", "param": "b"}"#).is_err());
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
            r#"{"symbol":"S","symbol_args":[],"extra":1}"#,
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
            foreign: None,
        };
        let json = serde_json::to_string(&ordinary).unwrap();
        assert!(!json.contains("is_error"));

        let sentinel = YieldsPos {
            name: "e".into(),
            r#type: None,
            is_error: true,
            foreign: None,
        };
        let json = serde_json::to_string(&sentinel).unwrap();
        assert!(json.contains(r#""is_error":true"#));
        let back: YieldsPos = serde_json::from_str(&json).unwrap();
        assert_eq!(back, sentinel);
    }

    #[test]
    fn extern_decl_omits_async_and_errors_when_empty_and_round_trips_them() {
        let lang = ExternLang {
            lang: "rust".into(),
            symbol: "load".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
        };
        let mut decl = ExternDecl {
            name: "load".into(),
            params: vec![],
            r#return: Tref::Prim(crate::ir::Prim::String),
            langs: vec![lang],
            r#async: vec![],
            errors: vec![],
        };
        let json = serde_json::to_string(&decl).unwrap();
        assert!(!json.contains("async"));
        assert!(!json.contains("errors"));
        assert!(!decl.is_async("rust"));
        decl.r#async = vec!["rust".into()];
        decl.errors = vec!["m#overloaded".into()];
        let json = serde_json::to_string(&decl).unwrap();
        assert!(json.contains(r#""async":["rust"]"#), "{json}");
        assert!(json.contains(r#""errors":["m#overloaded"]"#), "{json}");
        let back: ExternDecl = serde_json::from_str(&json).unwrap();
        assert_eq!(back, decl);
        assert!(back.is_async("rust") && !back.is_async("ts"));
    }

    #[test]
    fn yields_pos_foreign_spelling_round_trips() {
        let pos = YieldsPos {
            name: "c".into(),
            r#type: None,
            is_error: false,
            foreign: Some("ConstantCalculator<f64>".into()),
        };
        let json = serde_json::to_string(&pos).unwrap();
        assert!(
            json.contains(r#""foreign":"ConstantCalculator<f64>""#),
            "{json}"
        );
        let back: YieldsPos = serde_json::from_str(&json).unwrap();
        assert_eq!(back, pos);
    }

    #[test]
    fn foreign_lang_round_trips_and_reads_off_a_shape() {
        let mut fields = BTreeMap::new();
        fields.insert("message".to_string(), "Error()".to_string());
        let fl = ForeignLang {
            lang: "go".into(),
            name: "ErrParse".into(),
            fields,
        };
        let json = serde_json::to_value(&fl).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"lang": "go", "name": "ErrParse", "fields": {"message": "Error()"}})
        );
        let shape = crate::ir::Shape {
            id: "m#invalid_expression".into(),
            kind: crate::ir::ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits: vec![crate::ir::Trait {
                id: "foreign".into(),
                value: serde_json::json!([json]),
            }],
        };
        assert_eq!(ForeignLang::of_shape(&shape), vec![fl.clone()]);
        let module = crate::ir::Module {
            name: "m".into(),
            shapes: vec![shape],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
            tests: vec![],
        };
        assert_eq!(
            ForeignLang::of_error(&module, "m#invalid_expression", "go"),
            Some(fl)
        );
        assert_eq!(
            ForeignLang::of_error(&module, "m#invalid_expression", "ts"),
            None
        );
        assert_eq!(ForeignLang::of_error(&module, "m#nope", "go"), None);
        let bare = OpaqueType {
            name: "h".into(),
            langs: vec![],
            methods: vec![],
        };
        assert_eq!(bare.storage("go"), None);
    }
}

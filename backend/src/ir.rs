//! Rust mirror of the canonical IR. The OCaml frontend is the source of truth;
//! these types decode and re-encode the exact same JSON. The golden fixtures
//! under `ir-schema/fixtures/` are the arbiter, and the cross-language
//! round-trip test fails the build on any divergence.

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// IR schema revision this build understands. Bumped by one on every
/// incompatible change; there is no negotiation across versions.
pub const TONO_IR_VERSION: u32 = 1;

/// Closed primitive set. Serializes as a bare string ("i32", "string", ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Prim {
    Bool,
    String,
    Bytes,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Float,
    Timestamp,
    Date,
    Duration,
    Uuid,
}

/// Recursive type-application algebra. The wire form is a single-key tagged
/// object, except `ref`, which carries a sibling `args` array. This does not
/// match any uniform serde tagging mode, so the codec is hand-written.
#[derive(Debug, Clone, PartialEq)]
pub enum Tref {
    Prim(Prim),
    Ref { id: String, args: Vec<Tref> },
    Param(String),
    List(Box<Tref>),
    Map(Box<Tref>, Box<Tref>),
}

impl Serialize for Tref {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Tref::Prim(p) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("prim", p)?;
                m.end()
            }
            Tref::Param(x) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("param", x)?;
                m.end()
            }
            Tref::List(t) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("list", t)?;
                m.end()
            }
            Tref::Map(k, v) => {
                let mut m = s.serialize_map(Some(1))?;
                m.serialize_entry("map", &[k, v])?;
                m.end()
            }
            Tref::Ref { id, args } => {
                let mut m = s.serialize_map(Some(2))?;
                m.serialize_entry("ref", id)?;
                m.serialize_entry("args", args)?;
                m.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Tref {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        tref_from_value(&v).map_err(serde::de::Error::custom)
    }
}

const TREF_KEYS: [&str; 5] = ["prim", "ref", "param", "list", "map"];

fn ensure_only(obj: &serde_json::Map<String, Value>, allowed: &[&str]) -> Result<(), String> {
    match obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("unexpected key {k:?}")),
        None => Ok(()),
    }
}

fn prim_from_value(v: &Value) -> Result<Prim, String> {
    serde_json::from_value::<Prim>(v.clone()).map_err(|_| format!("unknown primitive {v}"))
}

fn tref_from_value(v: &Value) -> Result<Tref, String> {
    let obj = v.as_object().ok_or("expected an object")?;
    let present: Vec<&str> = TREF_KEYS
        .iter()
        .copied()
        .filter(|k| obj.contains_key(*k))
        .collect();
    match present.as_slice() {
        ["prim"] => {
            ensure_only(obj, &["prim"])?;
            Ok(Tref::Prim(prim_from_value(&obj["prim"])?))
        }
        ["param"] => {
            ensure_only(obj, &["param"])?;
            Ok(Tref::Param(
                obj["param"]
                    .as_str()
                    .ok_or("expected a string")?
                    .to_string(),
            ))
        }
        ["list"] => {
            ensure_only(obj, &["list"])?;
            Ok(Tref::List(Box::new(tref_from_value(&obj["list"])?)))
        }
        ["map"] => {
            ensure_only(obj, &["map"])?;
            let arr = obj["map"].as_array().ok_or("expected an array")?;
            if arr.len() != 2 {
                return Err("map expects a 2-element array".to_string());
            }
            Ok(Tref::Map(
                Box::new(tref_from_value(&arr[0])?),
                Box::new(tref_from_value(&arr[1])?),
            ))
        }
        ["ref"] => {
            ensure_only(obj, &["ref", "args"])?;
            let id = obj["ref"].as_str().ok_or("expected a string")?.to_string();
            let arr = obj
                .get("args")
                .ok_or("ref is missing args")?
                .as_array()
                .ok_or("expected an array")?;
            let args = arr.iter().map(tref_from_value).collect::<Result<_, _>>()?;
            Ok(Tref::Ref { id, args })
        }
        [] => Err("tref object has no recognized variant key".to_string()),
        _ => Err("tref object has multiple variant keys".to_string()),
    }
}

/// Core constraint vocabulary. Single-key tagged object with camelCase fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Constraint {
    Range {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        min: Option<f64>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        max: Option<f64>,
        #[serde(rename = "exclMin", default)]
        excl_min: bool,
        #[serde(rename = "exclMax", default)]
        excl_max: bool,
    },
    Length {
        #[serde(skip_serializing_if = "Option::is_none", default)]
        min: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        max: Option<i64>,
    },
    Pattern(String),
    MultipleOf(f64),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trait {
    pub id: String,
    pub value: Value,
}

fn default_true() -> bool {
    true
}

fn default_discriminator() -> String {
    "type".to_string()
}

// Distinguishes an absent key from a present `null`: absent -> None,
// `null` -> Some(None), value -> Some(Some(value)). serde's plain Option maps a
// present `null` to None, which would erase a deliberately-null default.
fn double_option<'de, D>(de: D) -> Result<Option<Option<Value>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Option::<Value>::deserialize(de)?))
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub name: String,
    pub target: Tref,
    pub required: bool,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "double_option"
    )]
    pub default: Option<Option<Value>>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub traits: Vec<Trait>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnumBacking {
    String,
    Int,
}

/// Shape kind, internally tagged by `kind` and flattened next to a shape's
/// `id` and `traits`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
// The optional-field defaults below mirror the frontend decoder's tolerance so
// both sides accept exactly the same documents; the encoder always writes every
// field, so these only matter for hand-authored or partial input.
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ShapeKind {
    Structure {
        #[serde(default)]
        params: Vec<String>,
        #[serde(default)]
        members: Vec<Member>,
    },
    Union {
        #[serde(default)]
        params: Vec<String>,
        #[serde(default)]
        members: Vec<Member>,
        #[serde(default = "default_discriminator")]
        discriminator: String,
    },
    Enum {
        backing: EnumBacking,
        #[serde(default)]
        values: Vec<(String, Option<i64>)>,
        #[serde(default = "default_true")]
        open: bool,
    },
    Service {
        #[serde(default)]
        operations: Vec<String>,
    },
    Operation {
        input: Option<Tref>,
        output: Option<Tref>,
        #[serde(default)]
        errors: Vec<Tref>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Shape {
    pub id: String,
    #[serde(flatten)]
    pub kind: ShapeKind,
    #[serde(default)]
    pub traits: Vec<Trait>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default)]
    pub operations: Vec<Shape>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub tono_ir_version: u32,
    #[serde(default)]
    pub modules: Vec<Module>,
}

#[derive(Deserialize)]
struct VersionEnvelope {
    tono_ir_version: u32,
}

/// Decode a model. The schema version is checked from the envelope *before*
/// decoding the rest, so an unrecognized version fails loudly with a version
/// error rather than a downstream parse error (matching the frontend order).
pub fn decode_model(json: &str) -> Result<Model, String> {
    let envelope: VersionEnvelope = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if envelope.tono_ir_version != TONO_IR_VERSION {
        return Err(format!(
            "unsupported tono_ir_version {} (this build supports {})",
            envelope.tono_ir_version, TONO_IR_VERSION
        ));
    }
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// Result of checking that a document survives a decode/re-encode round-trip.
pub struct RoundTrip {
    /// Whether the re-encoding equals the original document as data.
    pub equal: bool,
    /// The original document, parsed.
    pub original: Value,
    /// The document re-encoded from the decoded model.
    pub reencoded: Value,
}

/// Decode a document and re-encode it from the mirror, comparing the two as
/// JSON *data* rather than text: `serde_json::Value` equality is independent of
/// object key order and compares numbers by value, so it is immune to the
/// number-formatting differences between the frontend and backend emitters
/// while still catching any structural divergence (a renamed, extra, or missing
/// field changes the value tree).
pub fn check_roundtrip(json: &str) -> Result<RoundTrip, String> {
    let original: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let model = decode_model(json)?;
    let reencoded = serde_json::to_value(&model).map_err(|e| e.to_string())?;
    Ok(RoundTrip {
        equal: original == reencoded,
        original,
        reencoded,
    })
}

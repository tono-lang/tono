//! The declared-test surface of the IR mirror: constructions, stubs, calls,
//! and expect patterns. The frontend's `ir_json_tests.ml` codec is the source
//! of truth; construction values and call inputs are opaque wire-form JSON
//! carried verbatim, and only the pattern and answer structures are typed.

use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// One declared test: the clients it constructs, the dependency answers it
/// pins, the calls it makes, and what it expects of their outcomes. The
/// frontend always writes every array (empty when unused), so none is skipped
/// when empty: matching the encoder keeps the round-trip byte-for-byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestDecl {
    pub name: String,
    #[serde(default)]
    pub constructions: Vec<TestConstruction>,
    #[serde(default)]
    pub stubs: Vec<TestStub>,
    #[serde(default)]
    pub calls: Vec<TestCall>,
    #[serde(default)]
    pub expects: Vec<TestExpect>,
}

/// `binding = entry(values)`: construct an entry client under a binding. The
/// values are wire-form JSON per field, carried opaque.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestConstruction {
    pub binding: String,
    pub entry: String,
    #[serde(default)]
    pub values: BTreeMap<String, Value>,
}

/// Which dependency seam a stub replaces: the HTTP transport, or a bespoke
/// `impl` extension. Serializes as the lowercase word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StubDep {
    Http,
    Impl,
}

/// A pinned dependency for one client operation: each call consumes the next
/// answer in order. The binding names the stub so request expectations can
/// reference it; the frontend omits the key when the stub is unnamed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestStub {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    pub client: String,
    pub op: String,
    pub dep: StubDep,
    #[serde(default)]
    pub answers: Vec<StubAnswer>,
}

/// One canned answer. An HTTP answer is a bare status/headers/body object; an
/// impl answer wraps its single form (`value`, `error`, or `contract`) under
/// that key, so the variants are distinguished by their keys alone.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StubAnswer {
    Value { value: Value },
    Error { error: AnswerError },
    Contract { contract: Empty },
    Http(HttpAnswer),
}

/// A stubbed HTTP response. The frontend always writes all three fields and
/// tolerates their absence on decode, mirrored here by the defaults.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HttpAnswer {
    pub status: i64,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: String,
}

/// A stubbed declared error: the error shape's local name and its wire-form
/// data. The frontend defaults absent data to an empty object and always
/// writes it back, mirrored here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerError {
    pub shape: String,
    #[serde(default = "empty_object")]
    pub data: Value,
}

fn empty_object() -> Value {
    Value::Object(serde_json::Map::new())
}

/// `binding = client.op(input)`: one call under test. The input is wire-form
/// JSON carried opaque (its leaves may be `{"$ref": ...}` binding references);
/// the frontend omits the key when the op takes no input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TestCall {
    pub binding: String,
    pub client: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<Value>,
}

/// What a subject binding must satisfy: a single-key tagged pattern. `eq`
/// carries a wire-form JSON literal; `ok` asserts success without looking at
/// the value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TestPattern {
    Eq(Value),
    Struct(ShapePattern),
    Error(ShapePattern),
    Taxonomy(TaxonomyPattern),
    Ok(Empty),
}

/// A structural match against a declared shape (`struct` and `error` patterns
/// share this form). `open` tolerates fields the pattern does not mention. The
/// frontend always writes all three keys and defaults the last two on decode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapePattern {
    pub shape: String,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldPattern>,
}

/// A structural match against an error-taxonomy category; same form as
/// [`ShapePattern`] except the head key is `category`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaxonomyPattern {
    pub category: String,
    #[serde(default)]
    pub open: bool,
    #[serde(default)]
    pub fields: BTreeMap<String, FieldPattern>,
}

/// A per-field expectation inside a structural pattern: presence/absence
/// markers, or a nested pattern written directly (unwrapped).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldPattern {
    Absent { absent: Empty },
    Present { present: Empty },
    Pat(TestPattern),
}

/// The empty-object payload of marker keys (`{"ok":{}}`, `{"absent":{}}`,
/// `{"present":{}}`, `{"contract":{}}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Empty {}

/// One expectation: either a pattern over an outcome binding, or the recorded
/// requests of a stub binding. Distinguished by which sibling key `subject`
/// carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TestExpect {
    Outcome {
        subject: String,
        pattern: TestPattern,
    },
    Requests {
        subject: String,
        requests: Vec<RequestPattern>,
    },
}

/// A structural match against one recorded request. On the wire the headers
/// sub-pattern is folded into the `fields` object under the reserved
/// `headers` key, which no plain request field can use; the codec is
/// hand-written to keep the two apart in the model, as the frontend does.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPattern {
    pub open: bool,
    pub fields: BTreeMap<String, FieldPattern>,
    pub headers: Option<BTreeMap<String, FieldPattern>>,
}

impl Serialize for RequestPattern {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("open", &self.open)?;
        m.serialize_entry("fields", &RequestFields(self))?;
        m.end()
    }
}

/// Serializes the folded `fields` object: the plain fields, then the headers
/// sub-pattern under its reserved key (trailing, matching the frontend).
struct RequestFields<'a>(&'a RequestPattern);

impl Serialize for RequestFields<'_> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let extra = usize::from(self.0.headers.is_some());
        let mut m = s.serialize_map(Some(self.0.fields.len() + extra))?;
        for (name, pattern) in &self.0.fields {
            m.serialize_entry(name, pattern)?;
        }
        if let Some(headers) = &self.0.headers {
            m.serialize_entry("headers", headers)?;
        }
        m.end()
    }
}

impl<'de> Deserialize<'de> for RequestPattern {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        request_pattern_from_value(&v).map_err(serde::de::Error::custom)
    }
}

fn request_pattern_from_value(v: &Value) -> Result<RequestPattern, String> {
    let obj = v.as_object().ok_or("request pattern must be an object")?;
    let open = match obj.get("open") {
        None => false,
        Some(b) => b
            .as_bool()
            .ok_or("request pattern `open` must be a boolean")?,
    };
    let mut fields = BTreeMap::new();
    let mut headers = None;
    if let Some(fv) = obj.get("fields") {
        let entries = fv
            .as_object()
            .ok_or("request pattern `fields` must be an object")?;
        for (name, pv) in entries {
            if name == "headers" {
                let hs = pv
                    .as_object()
                    .ok_or("request pattern `headers` must be an object")?;
                let mut folded = BTreeMap::new();
                for (header, hv) in hs {
                    folded.insert(header.clone(), field_pattern_from_value(hv)?);
                }
                headers = Some(folded);
            } else {
                fields.insert(name.clone(), field_pattern_from_value(pv)?);
            }
        }
    }
    Ok(RequestPattern {
        open,
        fields,
        headers,
    })
}

fn field_pattern_from_value(v: &Value) -> Result<FieldPattern, String> {
    serde_json::from_value(v.clone()).map_err(|e| e.to_string())
}

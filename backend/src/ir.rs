//! Rust mirror of the canonical IR. The OCaml frontend is the source of truth;
//! these types decode and re-encode the exact same JSON. The golden fixtures
//! under `ir-schema/fixtures/` are the arbiter, and the cross-language
//! round-trip test fails the build on any divergence.

use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

// The declared-test types and the FFI ext/extern library types live in their
// own files to stay within the file-size gate; re-exporting them here keeps
// the IR mirror a single public surface.
pub use crate::ir_extern_model::*;
pub use crate::ir_tests_model::*;

/// IR schema revision this build understands. Bumped by one on every
/// incompatible change; there is no negotiation across versions.
/// v2 removed the enum `open` field (every enum is open).
/// v3 added the module `extensions` table (bespoke hooks/contracts/constraints).
/// v4 made an enum value an object carrying a trait bag (documentation rides it),
/// replacing the `[name, intOrNull]` pair.
/// v5 added the entry model: `entry`/`config` shape kinds whose fields carry
/// value sources, `@format` templates, transform pipelines, match selection,
/// and `@bind` composition; entry ops nest inside the entry shape and their
/// trait values may carry field references (`{"field": [...]}`).
/// v6 added the `impl` extension kind (a bespoke operation implementation) and
/// its optional `raw` flag.
/// v7 added the module `tests` array: declared tests (constructions, stubs,
/// calls, and expect patterns).
/// v8 added an operation's optional `wire` field: the resolved HTTP binding
/// (method, uri, per-member bindings, response bindings, success status
/// codes, and the entry-scoped endpoint/header/timeout/retry refs) as typed
/// structure, replacing the opaque wire_descriptor trait blob for direct
/// consumption (the blob itself still rode the trait bag, unchanged, for
/// backward compatibility).
/// v9 removed the wire_descriptor trait blob the frontend attached alongside
/// the typed `wire` field for backward compatibility; `wire` is the only
/// representation now.
/// v10 gave `@errorCode` a mandatory first argument: the dotted path to probe
/// in the error body, ahead of the discriminator value. The trait's IR value
/// is still a plain array of trait args, so the shape is unchanged; only its
/// arity is.
/// v11 changed the meaning of `WireBinding::success`: empty now means no
/// `code:` was declared (the 2xx-range convention applies), where every
/// prior version always populated it with at least the default `[200]`.
/// v12 named the op parameter (`Operation::input_name`, absent for the
/// legacy unnamed form) and unified the request-value grammar: `uri` and
/// `endpoint` became `WireValue` (literal, entry/param reference, or
/// template) instead of a bare template/path, `TemplatePart`/`WireValue`
/// gained a `Param` variant for a reference into the op's own parameter, and
/// `WireBinding` gained `query` (mirrors `request_headers`) for the new
/// op-level `@query` trait.
/// v13 removed the per-member HTTP binding traits (@httpLabel/@httpQuery/
/// @httpHeader/@httpPayload) and `WirePart`/`WireBinding::bindings`, the map
/// they resolved into: `WireBinding` gained `body` (a `WireValue`, absent
/// when the operation sends none), and `WireValue` gained an `Object`
/// variant (named fields, each a `WireValue`) for the new @body ctor-mapper
/// form.
/// v14 added the module `ext_libs` array: per-language module paths, foreign
/// struct/opaque-handle declarations, and `extern` declarations with a
/// per-language call/yields/returns/errors binding (surface-and-IR only; no
/// typecheck or codegen consumes it yet, see `ir_extern_model.rs`). An entry
/// field's `call` key (alongside the existing `select`) carries a field's
/// `= ns.fn(args)` extern-call source, and `CallArg` gained `lit`/`list`/
/// `call` variants for ctor-field values.
/// v15 added an operation's optional `impl_call` (`OpImplCall`): a third
/// implementation source (a bespoke `impl .field.method(args)` op
/// body), alongside a protocol's `wire` and a legacy `impl` extension. It
/// reuses the `lit` `CallArg` variant v14 introduced for ctor-field values,
/// now also reachable as a bare call argument -- no wire-shape change
/// there, only a new use site (`CallArg`'s codec already accepted `lit` at
/// any position). Surface-and-IR only at the time; every target's codegen
/// consumes `impl_call` since.
/// v17 added `WireValue::Call`: an extern call read as a @header/@query/
/// @body value. Its arguments mirror `CallArg`'s `field`/`param`/`lit`/
/// `ctor` tags, plus a reserved `request` tag for `.request`, the
/// canonical already-assembled request -- legal only in this position,
/// never during entry construction.
/// v18 added a declared test's `extern_stubs`: a stub for an `extern` FFI
/// call (free function or opaque-handle method), not tied to one client/op
/// like the existing HTTP/impl stub.
/// v21 added `OpaqueType::instance`: which instantiation of a foreign
/// generic type the handle names (the foreign type's own name, plus the
/// tono argument it is monomorphized with). Absent for a non-generic
/// foreign type.
/// v22 added an entry field's optional `handle_call` (`OpImplCall`, the
/// same shape as an operation's `impl_call`): a field whose value source
/// is a call into a sibling opaque handle's method, so one foreign
/// resolution feeds several operations.
pub const TONO_IR_VERSION: u32 = 22;

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

/// One member of an enum: its wire name, an optional explicit integer (present
/// only on int-backed enums, absent otherwise, mirroring the frontend encoder),
/// and its trait bag. Documentation (`@doc`) rides the bag exactly as it does on
/// shapes and struct members, so codegen reads it through the same path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnumValue {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<i64>,
    #[serde(default)]
    pub traits: Vec<Trait>,
}

/// Where an entry/config field's value can come from; the declared order is
/// the fallback chain. Wire form: `"arg"`, `"with"`, `{"env": <name>}`, or
/// `{"default": <json>}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Arg,
    With,
    Env(EnvName),
    Default(Value),
}

/// The `@env` argument: a literal variable name, or a sibling-field reference
/// whose resolved value names the variable (`{"field": [...]}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum EnvName {
    Name(String),
    Field(FieldRef),
}

/// A structured entry-field reference, `{"field": ["a", "b"]}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldRef {
    pub field: Vec<String>,
}

/// One piece of a parsed template: a literal run, an entry-field placeholder
/// (`{.x}`), an operation-parameter placeholder (`{.x}` whose head segment
/// names the op's declared parameter instead of an entry field; the
/// remaining segments index into it, empty for the whole parameter), or the
/// legacy operation-input member placeholder (`{id}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TemplatePart {
    Lit(String),
    Field(Vec<String>),
    Param(Vec<String>),
    Input(String),
}

/// What a selected match arm yields: another field, a literal, or a stack of
/// sources resolved in place.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArmValue {
    Field(Vec<String>),
    Lit(Value),
    Sources(Vec<Source>),
    /// "._": the match subject itself, narrowed non-optional. A payload-less
    /// variant serializes as a bare tag string ("subject"), which the OCaml
    /// mirror's encoder/decoder match exactly for round-trip.
    Subject,
}

/// One arm of a match selection; an absent `pattern` is the wildcard arm, a
/// present JSON `null` the mandatory "null" arm of an optional subject.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SelectArm {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pattern: Option<Value>,
    pub value: ArmValue,
}

/// The selection table of `field: T = match .subject { ... }`. `subject_index`,
/// when present, is a single-level map-index key path applied to `subject`
/// (".cfg.by_segment[.seg]"), making the subject optional.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Select {
    pub subject: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject_index: Option<Vec<String>>,
    #[serde(default)]
    pub arms: Vec<SelectArm>,
}

/// One `@bind(target, .source)` at a composition point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bind {
    pub field: String,
    pub source: Vec<String>,
}

/// One field of an entry or config. Presence is governed by the sources, so
/// there is no required/default pair here (`@default` is a source).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntryField {
    pub name: String,
    pub target: Tref,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<Vec<TemplatePart>>,
    #[serde(default)]
    pub transforms: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<Select>,
    /// A `= ns.fn(args)` extern-call source, alongside the existing `select`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub call: Option<EntryCall>,
    /// A `= .field.method(args)` handle-method-call source: the
    /// receiver-form counterpart of `call`, sharing the op `impl` node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handle_call: Option<OpImplCall>,
    #[serde(default)]
    pub binds: Vec<Bind>,
    #[serde(default)]
    pub constraints: Vec<Constraint>,
    #[serde(default)]
    pub traits: Vec<Trait>,
}

/// Where an output member is read from in the HTTP response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum WireResponsePart {
    Header {
        name: String,
    },
    #[serde(rename = "statusCode")]
    StatusCode,
}

/// A value position in a protocol trait: a literal, an entry-field
/// reference, an op-parameter reference (segments into the op's declared
/// parameter, empty for the whole value), a template mixing literal runs
/// with entry-field/op-parameter placeholders, or an object (the @body
/// ctor mapper: named fields, each itself a `WireValue`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireValue {
    Lit(Value),
    Field(Vec<String>),
    Param(Vec<String>),
    Template(Vec<TemplatePart>),
    Object(Vec<(String, WireValue)>),
    /// An extern call read as a @header/@query/@body value.
    Call(WireCall),
}

/// An extern call bound as a @header/@query/@body value: `ns`/`fn` name the
/// `extern` declared in the module's own `ext_libs`, `args` are its
/// arguments.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireCall {
    pub ns: String,
    #[serde(rename = "fn")]
    pub fn_name: String,
    pub args: Vec<WireCallArg>,
}

/// One argument to a wire-position extern call: a ref into the op's own
/// scope (`Field`/`Param`, matching `WireValue`'s own), a literal, a
/// struct-literal mapper, or the reserved `Request` marker for `.request`,
/// the canonical, already-assembled request -- legal only here, never
/// during entry construction (see `CallArg` for the entry-construction
/// counterpart, which has no such marker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WireCallArg {
    Field(Vec<String>),
    Param(Vec<String>),
    Lit(Value),
    Ctor(Vec<(String, WireCallArg)>),
    Request,
}

/// The resolved HTTP binding a Protocol pass computes once and a Target
/// reads directly: the typed counterpart of the wire_descriptor blob. Absent
/// (`None`) for an operation with no `@http` trait. `response_bindings` is
/// keyed by member name (unique by construction), matching the map
/// convention already used for `Extension::bindings`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireBinding {
    pub method: String,
    #[serde(default = "WireBinding::default_uri")]
    pub uri: WireValue,
    /// The request body, absent when the operation sends none (never
    /// inferred from what the input leaves unmarked).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<WireValue>,
    #[serde(default)]
    pub response_bindings: BTreeMap<String, WireResponsePart>,
    /// The declared success statuses. Empty when the operation left `code:`
    /// unset: the 2xx-range convention applies. Non-empty means an exact
    /// match against these statuses only, whether or not they fall in 2xx.
    #[serde(default)]
    pub success: Vec<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<WireValue>,
    #[serde(default)]
    pub request_headers: Vec<(Vec<TemplatePart>, WireValue)>,
    #[serde(default)]
    pub query: Vec<(Vec<TemplatePart>, WireValue)>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Vec<String>>,
}

impl WireBinding {
    fn default_uri() -> WireValue {
        WireValue::Template(Vec::new())
    }
}

/// A call into an entry field's declared opaque-handle method,
/// `.field.method(args)`: an op's own `impl` body, or a field's own
/// `handle_call` value source. Mirrors `EntryCall` with `recv: Vec<String>`
/// (a field path) in place of `ns` (a bare `ext` namespace), since the
/// receiver is an entry field, not an extern namespace. Resolving the
/// receiver/method against a declared handle is a typechecker concern on
/// the frontend side; this only carries the call structured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpImplCall {
    pub recv: Vec<String>,
    pub method: String,
    #[serde(default)]
    pub args: Vec<CallArg>,
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
        values: Vec<EnumValue>,
    },
    Service {
        #[serde(default)]
        operations: Vec<String>,
    },
    Operation {
        input: Option<Tref>,
        // The declared parameter name; absent for the legacy unnamed form,
        // where `.input` has no user-given name to resolve.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_name: Option<String>,
        output: Option<Tref>,
        #[serde(default)]
        errors: Vec<Tref>,
        // Boxed: WireBinding is far larger than the other Operation fields, and
        // an unboxed Option<WireBinding> would inflate ShapeKind's overall size
        // for every non-Operation variant too (clippy::large_enum_variant).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wire: Option<Box<WireBinding>>,
        /// The op's own `impl .field.method(args)` body: a third
        /// implementation source alongside `wire` and a legacy `impl`
        /// extension.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        impl_call: Option<OpImplCall>,
    },
    /// A struct with ops in its body: the SDK construction surface plus its
    /// methods; never a wire type.
    Entry {
        #[serde(default)]
        fields: Vec<EntryField>,
        #[serde(default)]
        operations: Vec<Shape>,
    },
    /// A construction-only struct (its fields carry sources, or an entry
    /// composes it via `@bind`); never a wire type.
    Config {
        #[serde(default)]
        fields: Vec<EntryField>,
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

/// A bespoke extension bound to per-language source files. `Contract`/`Constraint`
/// are named with a typed signature; `Impl` implements the operation its `name`
/// points at (bare, or `entry.op` when the bare name would be ambiguous) and
/// takes that operation's signature. Serializes as the lowercase word under a
/// `kind` key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtKind {
    Contract,
    Constraint,
    Impl,
}

/// A contract/constraint boundary: the input and output type refs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Signature {
    pub input: Tref,
    pub output: Tref,
}

/// One extension-table entry. `bindings` maps a language to its
/// `ext/{lang}/...#symbol` reference; `conformance` is the vector reference the
/// generator requires for a contract. The optional-field defaults mirror the
/// frontend encoder (which omits an absent signature/conformance) so both sides
/// round-trip the same bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Extension {
    pub name: String,
    pub kind: ExtKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<Signature>,
    /// Impl only: the bound symbol returns a raw outcome the generated glue
    /// decodes, instead of the operation's declared output. The typed form is
    /// the default, so the frontend omits the key when it is false.
    #[serde(default, skip_serializing_if = "is_false")]
    pub raw: bool,
    #[serde(default)]
    pub bindings: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conformance: Option<String>,
}

fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Module {
    pub name: String,
    #[serde(default)]
    pub shapes: Vec<Shape>,
    #[serde(default)]
    pub operations: Vec<Shape>,
    // The frontend always writes this array (empty when there are no
    // extensions), so it is not skipped when empty: matching the encoder keeps
    // the cross-language round-trip byte-for-byte.
    #[serde(default)]
    pub extensions: Vec<Extension>,
    // Always written by the frontend for the same round-trip reason as
    // `extensions`.
    #[serde(default)]
    pub ext_libs: Vec<ExtLib>,
    #[serde(default)]
    pub tests: Vec<TestDecl>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub tono_ir_version: u32,
    #[serde(default)]
    pub modules: Vec<Module>,
}

/// Decode a model. The JSON is parsed once; the schema version is checked from
/// the parsed value *before* the model is built, so an unrecognized version
/// fails loudly with a version error rather than a downstream field error
/// (matching the frontend order).
pub fn decode_model(json: &str) -> Result<Model, String> {
    let value: Value = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let version = value
        .get("tono_ir_version")
        .and_then(Value::as_u64)
        .ok_or("model is missing an integer tono_ir_version")?;
    if version != u64::from(TONO_IR_VERSION) {
        return Err(format!(
            "unsupported tono_ir_version {version} (this build supports {TONO_IR_VERSION})"
        ));
    }
    serde_json::from_value(value).map_err(|e| e.to_string())
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

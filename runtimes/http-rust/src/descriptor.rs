//! The opaque wire descriptor a tono-generated Rust SDK embeds verbatim. This
//! runtime is the only layer that reads it: it executes the descriptor without
//! re-deriving any default and raises no typed error (the generated SDK maps
//! the [`crate::runtime::Outcome`] onto its own taxonomy).

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// Where one input member goes in the HTTP request: `"label"` (substitutes
/// `{member-name}` in the uri), `"query"`, `"header"`, `"body"` (a field inside
/// the JSON body), or `"payload"` (the member is the whole body, no envelope).
/// `name` carries the query/header wire name.
#[derive(Debug, Clone, Deserialize)]
pub struct Part {
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// One input member's [`Part`], read off the compiler's `["member", {...}]`
/// pair.
#[derive(Debug, Clone)]
pub struct Binding {
    pub member: String,
    pub part: Part,
}

impl<'de> Deserialize<'de> for Binding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (member, part) = <(String, Part)>::deserialize(deserializer)?;
        Ok(Binding { member, part })
    }
}

/// Where one output member is read from in the HTTP response: `"header"`
/// (with `name`) or `"statusCode"`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResponsePart {
    pub kind: String,
    #[serde(default)]
    pub name: String,
}

/// One output member's [`ResponsePart`], read off the `["member", {...}]`
/// pair.
#[derive(Debug, Clone)]
pub struct ResponseBinding {
    pub member: String,
    pub part: ResponsePart,
}

impl<'de> Deserialize<'de> for ResponseBinding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (member, part) = <(String, ResponsePart)>::deserialize(deserializer)?;
        Ok(ResponseBinding { member, part })
    }
}

/// One declared success status. The wire's second element (the output type
/// ref) is opaque to the runtime: the SDK owns decoding, the runtime uses only
/// the status to decide success vs error.
#[derive(Debug, Clone)]
pub struct SuccessCase {
    pub status: i64,
}

impl<'de> Deserialize<'de> for SuccessCase {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let parts = Vec::<Value>::deserialize(deserializer)?;
        let status = parts
            .first()
            .ok_or_else(|| D::Error::custom("success case is not an array"))?;
        let status = status
            .as_i64()
            .ok_or_else(|| D::Error::custom("success status is not an integer"))?;
        Ok(SuccessCase { status })
    }
}

/// One piece of a template position: a literal run, an entry-field
/// placeholder resolved from the client values by its dotted path, or an
/// operation-input placeholder resolved from the call's input record. Exactly
/// one of the three is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TemplatePart {
    #[serde(default)]
    pub lit: Option<String>,
    #[serde(default)]
    pub field: Option<Vec<String>>,
    #[serde(default)]
    pub input: Option<String>,
}

/// A descriptor value position: a literal frozen by the compiler, an
/// entry-field reference resolved from the client values, or a template of
/// parts. Exactly one of the three is set.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ValueExpr {
    #[serde(default)]
    pub lit: Option<Value>,
    #[serde(default)]
    pub field: Option<Vec<String>>,
    #[serde(default)]
    pub template: Option<Vec<TemplatePart>>,
}

/// One declared request header: its name as a template and its value
/// expression, read off the compiler's `[[part...], valueExpr]` pair.
#[derive(Debug, Clone)]
pub struct RequestHeader {
    pub key: Vec<TemplatePart>,
    pub value: ValueExpr,
}

impl<'de> Deserialize<'de> for RequestHeader {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (key, value) = <(Vec<TemplatePart>, ValueExpr)>::deserialize(deserializer)?;
        Ok(RequestHeader { key, value })
    }
}

/// A descriptor position that yields a number at call time: either a literal
/// frozen by the compiler (`lit`) or a reference to a resolved client field
/// looked up by canonical name (`ref`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ValueSource {
    #[serde(default)]
    pub r#ref: Option<String>,
    #[serde(default)]
    pub lit: Option<f64>,
}

/// The operation retries, with the maximum number of retries (attempts after
/// the first) read from `max`.
#[derive(Debug, Clone, Deserialize)]
pub struct RetrySpec {
    pub max: ValueSource,
}

/// The opaque, language-agnostic shape the tono HTTP Protocol produces for
/// each operation and the generated SDK embeds verbatim.
#[derive(Debug, Clone, Deserialize)]
pub struct WireDescriptor {
    pub http_method: String,
    pub uri: String,
    #[serde(default)]
    pub bindings: Vec<Binding>,
    #[serde(default)]
    pub response_bindings: Vec<ResponseBinding>,
    #[serde(default)]
    pub success: Vec<SuccessCase>,
    /// Absent when the operation declares no retry: absent means one attempt,
    /// ever.
    #[serde(default)]
    pub retry: Option<RetrySpec>,
    /// The per-attempt budget in milliseconds; absent means no per-attempt
    /// deadline.
    #[serde(default)]
    pub timeout: Option<ValueSource>,
    /// Names the resolved client field (by path) whose value is the base URL
    /// for this operation; empty means `Options::base_url`.
    #[serde(default)]
    pub endpoint: Vec<String>,
    /// The operation's declared headers, applied before the caller's
    /// `Options::headers` so a bespoke or caller-supplied header wins.
    #[serde(default)]
    pub request_headers: Vec<RequestHeader>,
}

impl WireDescriptor {
    /// Decode the JSON descriptor literal embedded by the generated SDK.
    pub fn parse(data: &str) -> Result<WireDescriptor, serde_json::Error> {
        serde_json::from_str(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_minimal_descriptor() {
        let d = WireDescriptor::parse(
            r#"{"http_method":"POST","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]]}"#,
        )
        .unwrap();
        assert_eq!(d.http_method, "POST");
        assert_eq!(d.uri, "/x");
        assert_eq!(d.success[0].status, 200);
        assert!(d.retry.is_none());
        assert!(d.timeout.is_none());
    }

    #[test]
    fn a_binding_reads_the_member_part_pair() {
        let d = WireDescriptor::parse(
            r#"{"http_method":"GET","uri":"/x/{id}","bindings":[["id",{"kind":"label"}]],"response_bindings":[],"success":[]}"#,
        )
        .unwrap();
        assert_eq!(d.bindings[0].member, "id");
        assert_eq!(d.bindings[0].part.kind, "label");
    }

    #[test]
    fn a_request_header_reads_the_key_value_pair() {
        let d = WireDescriptor::parse(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[],"request_headers":[[[{"lit":"X-Client"}],{"field":["client_name"]}]]}"#,
        )
        .unwrap();
        let h = &d.request_headers[0];
        assert_eq!(h.key[0].lit.as_deref(), Some("X-Client"));
        assert_eq!(
            h.value.field.as_deref(),
            Some(["client_name".to_string()].as_slice())
        );
    }

    #[test]
    fn a_ref_value_source_and_a_lit_value_source_both_parse() {
        let d = WireDescriptor::parse(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[],"retry":{"max":{"ref":"max_retries"}},"timeout":{"lit":30}}"#,
        )
        .unwrap();
        assert_eq!(d.retry.unwrap().max.r#ref.as_deref(), Some("max_retries"));
        assert_eq!(d.timeout.unwrap().lit, Some(30.0));
    }

    #[test]
    fn a_malformed_descriptor_fails_to_parse() {
        assert!(WireDescriptor::parse("not json").is_err());
        assert!(WireDescriptor::parse(r#"{"uri":"/x"}"#).is_err());
    }
}

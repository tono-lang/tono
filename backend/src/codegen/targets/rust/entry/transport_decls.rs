//! The declaration text behind the emitted transport: the public,
//! bespoke-facing support types (`Group::root_support()`) and the internal
//! helper functions (`Group::root("http")`). Split from `transport.rs` (the
//! per-operation emission) to stay within the repo's file-size ceiling; the
//! pruning story is unchanged, since every declaration still names itself
//! and rides the usual group mechanics.

use crate::codegen::tree::Decl;

/// A support declaration whose text references nothing outside itself.
fn plain(name: &str, text: &str) -> Decl {
    Decl::raw_providing(name, text, Vec::new())
}

/// The public, bespoke-facing transport types (`Group::root_support()`): the
/// request/response shapes a bound `before_request`/`after_response` hook and
/// a canonical transport type against, plus the frozen `ClientOptions`.
pub(crate) fn http_support_decls() -> Vec<Decl> {
    let mut decls: Vec<Decl> = [
        (
            "BoxFuture",
            "/// The boxed, `Send` future every async transport seam returns (a\n\
             /// canonical transport's call, the retry sleep seam).\n\
             pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;",
        ),
        (
            "HttpError",
            "/// The error a transport attempt (or a lifecycle hook) fails with; the\n\
             /// generated client classifies it into its own error taxonomy.\n\
             pub type HttpError = Box<dyn std::error::Error + Send + Sync>;",
        ),
        (
            "HttpRequest",
            "/// HttpRequest is the request the generated client builds before sending\n\
             /// it. A before_request hook receives it and may return a mutated copy\n\
             /// (set an auth header, sign the body). `body` is `None` when the\n\
             /// request carries no body.\n\
             #[derive(Clone, Debug)]\n\
             pub struct HttpRequest {\n    pub method: String,\n    pub url: String,\n    pub headers: std::collections::HashMap<String, String>,\n    pub body: Option<String>,\n}",
        ),
        (
            "HttpResponse",
            "/// HttpResponse is the response the generated client reads before\n\
             /// classifying it. Header keys are lowercased (HTTP header names are\n\
             /// case-insensitive). An after_response hook may return a mutated copy.\n\
             #[derive(Clone, Debug)]\n\
             pub struct HttpResponse {\n    pub status: u16,\n    pub headers: std::collections::HashMap<String, String>,\n    pub body: String,\n}",
        ),
    ]
    .into_iter()
    .map(|(name, text)| plain(name, text))
    .collect();
    decls.push(Decl::raw_providing(
        "HttpTransport",
        "/// HttpTransport adapts any HTTP stack by mapping HttpRequest to\n\
         /// HttpResponse, without emulating `reqwest`. One call is one attempt:\n\
         /// the generated client owns retry, so a transport with internal\n\
         /// retries does not combine with it.\n\
         pub type HttpTransport = std::sync::Arc<\n    dyn Fn(HttpRequest) -> BoxFuture<'static, Result<HttpResponse, HttpError>> + Send + Sync,\n>;",
        vec![
            super::support_symbol("HttpRequest"),
            super::support_symbol("HttpResponse"),
            super::support_symbol("HttpError"),
            super::support_symbol("BoxFuture"),
        ],
    ));
    decls.push(Decl::raw_providing(
        "ClientOptions",
        "/// ClientOptions is what construction froze off the resolved Settings,\n\
         /// shared across every operation. Exactly one transport slot may be\n\
         /// set: `client` (native `reqwest`, present only with the crate's\n\
         /// default-on `reqwest` feature) or `transport` (canonical); setting\n\
         /// both is a construction error, and with the feature off the\n\
         /// canonical slot is required. No slot ships its own auth; a bespoke\n\
         /// hook sets an auth header through `headers`.\n\
         pub struct ClientOptions {\n    #[cfg(feature = \"reqwest\")]\n    pub client: Option<reqwest::Client>,\n    pub transport: Option<HttpTransport>,\n    pub headers: std::collections::HashMap<String, String>,\n}",
        vec![super::support_symbol("HttpTransport")],
    ));
    decls
}

/// The internal transport helpers (`Group::root("http")`), pruned SDK-wide by
/// usage exactly like the `duration`/`casing` groups: an SDK with no `@retry`
/// anywhere never carries the backoff helpers.
pub(crate) fn internal_helpers() -> Vec<Decl> {
    let support = |name: &str| super::support_symbol(name);
    let mut decls: Vec<Decl> = [
        (
            "format_scalar",
            "/// Renders a record member's raw JSON value the way the wire expects it\n\
             /// in a path, query, or header position: a JSON string unquoted,\n\
             /// anything else verbatim, so a wide integer or a formatting-sensitive\n\
             /// float keeps the exact spelling its own encoder gave it. An absent\n\
             /// or null value renders empty.\n\
             pub fn format_scalar(v: Option<&serde_json::value::RawValue>) -> String {\n\
             \x20   let Some(v) = v else { return String::new() };\n\
             \x20   let text = v.get();\n\
             \x20   if text == \"null\" {\n\
             \x20       return String::new();\n\
             \x20   }\n\
             \x20   if let Some(inner) = text.strip_prefix('\"').and_then(|s| s.strip_suffix('\"')) {\n\
             \x20       // The common case (no escape sequence) slices the quotes off\n\
             \x20       // directly; only a string carrying an escape pays for the decoder.\n\
             \x20       if !inner.contains('\\\\') {\n\
             \x20           return inner.to_string();\n\
             \x20       }\n\
             \x20       return serde_json::from_str::<String>(text).unwrap_or_default();\n\
             \x20   }\n\
             \x20   text.to_string()\n\
             }",
        ),
        (
            "percent_encode",
            "/// Percent-encode with the shared safe set (unreserved characters\n\
             /// only), matching the other targets' path and query escaping closely\n\
             /// enough for the wire contract.\n\
             fn percent_encode(s: &str) -> String {\n\
             \x20   let mut out = String::with_capacity(s.len());\n\
             \x20   for b in s.as_bytes() {\n\
             \x20       if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {\n\
             \x20           out.push(*b as char);\n\
             \x20       } else {\n\
             \x20           out.push_str(&format!(\"%{b:02X}\"));\n\
             \x20       }\n\
             \x20   }\n\
             \x20   out\n\
             }",
        ),
        (
            "percent_path",
            "/// One path segment's rendered value, percent-encoded.\n\
             pub fn percent_path(s: &str) -> String {\n    percent_encode(s)\n}",
        ),
        (
            "path_part",
            "/// A path segment read off the input record: an absent or null value\n\
             /// substitutes empty rather than a literal \"null\".\n\
             pub fn path_part(v: Option<&serde_json::value::RawValue>) -> String {\n\
             \x20   percent_path(&format_scalar(v))\n\
             }",
        ),
        (
            "set_header",
            "/// Overrides across casings: header names are case-insensitive, so a\n\
             /// bespoke \"authorization\" replaces a declared \"Authorization\" rather\n\
             /// than riding beside it.\n\
             pub fn set_header(headers: &mut std::collections::HashMap<String, String>, name: &str, value: String) {\n\
             \x20   headers.retain(|k, _| !k.eq_ignore_ascii_case(name));\n\
             \x20   headers.insert(name.to_string(), value);\n\
             }",
        ),
        (
            "has_header",
            "/// Whether a header is already set under any casing: a caller-supplied\n\
             /// \"Content-Type\" must suppress the default rather than sit beside a\n\
             /// second \"content-type\".\n\
             pub fn has_header(headers: &std::collections::HashMap<String, String>, name: &str) -> bool {\n\
             \x20   headers.keys().any(|k| k.eq_ignore_ascii_case(name))\n\
             }",
        ),
        (
            "append_query",
            "/// Serializes a record member's raw JSON value as a repeated entry per\n\
             /// element for a list, a single entry otherwise; an absent or null\n\
             /// value is omitted (the body's nullable-omit rule, applied to the\n\
             /// request line). A malformed array binds as a single entry rather\n\
             /// than failing the request line.\n\
             pub fn append_query(query: &mut Vec<String>, name: &str, value: Option<&serde_json::value::RawValue>) {\n\
             \x20   let Some(value) = value else { return };\n\
             \x20   let text = value.get();\n\
             \x20   if text == \"null\" {\n\
             \x20       return;\n\
             \x20   }\n\
             \x20   let mut push = |v: &serde_json::value::RawValue| {\n\
             \x20       query.push(format!(\"{}={}\", percent_encode(name), percent_encode(&format_scalar(Some(v)))));\n\
             \x20   };\n\
             \x20   if text.starts_with('[') {\n\
             \x20       if let Ok(elements) = serde_json::from_str::<Vec<Box<serde_json::value::RawValue>>>(text) {\n\
             \x20           for element in &elements {\n\
             \x20               push(element);\n\
             \x20           }\n\
             \x20           return;\n\
             \x20       }\n\
             \x20   }\n\
             \x20   push(value);\n\
             }",
        ),
        (
            "parse_json_object",
            "/// Parses a response body for response-bound member folding; a\n\
             /// non-object or unparsable body leaves the bound fields to stand on\n\
             /// their own.\n\
             pub fn parse_json_object(body: &str) -> serde_json::Map<String, serde_json::Value> {\n\
             \x20   serde_json::from_str(body).unwrap_or_default()\n\
             }",
        ),
        (
            "encode_record",
            "/// Encodes a typed input into the wire record the request positions\n\
             /// bind from: one encode pass, then a split by member name. Each\n\
             /// member is held as its own raw JSON bytes, so a request position\n\
             /// reads a value with the exact spelling and precision its own\n\
             /// encoder gave it, without decoding it into a generic tree.\n\
             pub fn encode_record<T: serde::Serialize>(\n\
             \x20   v: &T,\n\
             ) -> Result<std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>, serde_json::Error> {\n\
             \x20   serde_json::from_str(&serde_json::to_string(v)?)\n\
             }",
        ),
        (
            "encode_body",
            "/// Assembles the body-bound members into a JSON object by\n\
             /// concatenating their raw bytes, in the given member order: no\n\
             /// member is ever decoded and re-encoded, so a value reaches the wire\n\
             /// with the exact spelling and precision its own encoder gave it. A\n\
             /// member that is present but null still lands in the object as\n\
             /// null; only absence omits it. `None` when no member is present.\n\
             pub fn encode_body(\n\
             \x20   record: &std::collections::BTreeMap<String, Box<serde_json::value::RawValue>>,\n\
             \x20   members: &[&str],\n\
             ) -> Option<String> {\n\
             \x20   let mut fields = String::new();\n\
             \x20   for member in members {\n\
             \x20       let Some(raw) = record.get(*member) else { continue };\n\
             \x20       if fields.is_empty() {\n\
             \x20           fields.push('{');\n\
             \x20       } else {\n\
             \x20           fields.push(',');\n\
             \x20       }\n\
             \x20       // Serializing a string cannot fail, so the key carries no error path.\n\
             \x20       fields.push_str(&serde_json::to_string(member).unwrap_or_default());\n\
             \x20       fields.push(':');\n\
             \x20       fields.push_str(raw.get());\n\
             \x20   }\n\
             \x20   if fields.is_empty() {\n\
             \x20       return None;\n\
             \x20   }\n\
             \x20   fields.push('}');\n\
             \x20   Some(fields)\n\
             }",
        ),
        (
            "backoff_delay_ms",
            "/// Exponential backoff with full jitter: the constants are part of the\n\
             /// cross-runtime parity contract and must match every other target.\n\
             pub fn backoff_delay_ms(attempt: u32, random: f64) -> f64 {\n\
             \x20   random * 2000f64.min(100.0 * 2f64.powi(attempt as i32))\n\
             }",
        ),
        (
            "resolve_max_retries",
            "/// Clamps the operation's `@retry` field, which the frontend types as\n\
             /// an integer scalar: a value below one means zero retries.\n\
             pub fn resolve_max_retries(value: i64) -> u32 {\n\
             \x20   value.clamp(0, u32::MAX as i64) as u32\n\
             }",
        ),
        (
            "default_sleep",
            "pub fn default_sleep() -> SleepFn {\n\
             \x20   std::sync::Arc::new(|ms: f64| Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0))))\n\
             }",
        ),
        (
            "RandomFn",
            "/// The jitter source behind the backoff, swappable like the sleep.\n\
             pub type RandomFn = std::sync::Arc<dyn Fn() -> f64 + Send + Sync>;",
        ),
        (
            "default_random",
            "/// A \"random-enough\" `[0, 1)` value for backoff jitter, drawn from the\n\
             /// standard library's per-process random seed (no `rand` dependency;\n\
             /// jitter has no security requirement, and the parity harness always\n\
             /// pins the seam).\n\
             pub fn default_random() -> RandomFn {\n\
             \x20   std::sync::Arc::new(|| {\n\
             \x20       use std::collections::hash_map::RandomState;\n\
             \x20       use std::hash::{BuildHasher, Hasher};\n\
             \x20       let seed = RandomState::new().build_hasher().finish();\n\
             \x20       (seed as f64 / u64::MAX as f64).min(0.999_999_999)\n\
             \x20   })\n\
             }",
        ),
    ]
    .into_iter()
    .map(|(name, text)| plain(name, text))
    .collect();
    decls.push(Decl::raw_providing(
        "check_transport",
        "/// Rejects an unusable transport configuration at construction time:\n\
         /// with the `reqwest` feature on, setting both slots is ambiguous;\n\
         /// with it off, the canonical slot is the only way to send at all.\n\
         #[cfg(feature = \"reqwest\")]\n\
         pub fn check_transport(options: &ClientOptions) -> Result<(), String> {\n\
         \x20   if options.client.is_some() && options.transport.is_some() {\n\
         \x20       return Err(\"Settings.client and Settings.transport are mutually exclusive: set the native slot or the canonical slot, not both\".to_string());\n\
         \x20   }\n\
         \x20   Ok(())\n\
         }\n\
         #[cfg(not(feature = \"reqwest\"))]\n\
         pub fn check_transport(options: &ClientOptions) -> Result<(), String> {\n\
         \x20   if options.transport.is_none() {\n\
         \x20       return Err(\"no transport configured: enable the SDK crate's reqwest feature or set the canonical transport slot\".to_string());\n\
         \x20   }\n\
         \x20   Ok(())\n\
         }",
        vec![support("ClientOptions")],
    ));
    decls.push(Decl::raw_providing(
        "http_send",
        "/// One attempt: the canonical transport when set, otherwise the native\n\
         /// `reqwest` stack (a fresh default client when none was configured).\n\
         pub async fn http_send(options: &ClientOptions, request: HttpRequest) -> Result<HttpResponse, HttpError> {\n\
         \x20   if let Some(transport) = &options.transport {\n\
         \x20       return transport(request).await;\n\
         \x20   }\n\
         \x20   native_send(options, request).await\n\
         }\n\
         #[cfg(feature = \"reqwest\")]\n\
         async fn native_send(options: &ClientOptions, request: HttpRequest) -> Result<HttpResponse, HttpError> {\n\
         \x20   let client = options.client.clone().unwrap_or_default();\n\
         \x20   let mut builder = client.request(request.method.parse::<reqwest::Method>()?, &request.url);\n\
         \x20   for (name, value) in &request.headers {\n\
         \x20       builder = builder.header(name.as_str(), value.as_str());\n\
         \x20   }\n\
         \x20   if let Some(body) = request.body {\n\
         \x20       builder = builder.body(body);\n\
         \x20   }\n\
         \x20   let response = builder.send().await?;\n\
         \x20   let status = response.status().as_u16();\n\
         \x20   let headers: std::collections::HashMap<String, String> = response\n\
         \x20       .headers()\n\
         \x20       .iter()\n\
         \x20       .map(|(k, v)| (k.as_str().to_lowercase(), v.to_str().unwrap_or_default().to_string()))\n\
         \x20       .collect();\n\
         \x20   let body = response.text().await?;\n\
         \x20   Ok(HttpResponse { status, headers, body })\n\
         }\n\
         #[cfg(not(feature = \"reqwest\"))]\n\
         async fn native_send(_options: &ClientOptions, _request: HttpRequest) -> Result<HttpResponse, HttpError> {\n\
         \x20   Err(\"no transport configured: enable the SDK crate's reqwest feature or set the canonical transport slot\".into())\n\
         }",
        vec![
            support("ClientOptions"),
            support("HttpRequest"),
            support("HttpResponse"),
            support("HttpError"),
        ],
    ));
    decls.push(Decl::raw_providing(
        "http_send_with_timeout",
        "/// Bounds one attempt: the deadline covers the transport call only\n\
         /// (`tokio::time::timeout` drops the attempt's future, which is Rust's\n\
         /// native cancellation), so a transport that never answers still\n\
         /// times out. A non-positive deadline means no deadline.\n\
         pub async fn http_send_with_timeout(options: &ClientOptions, request: HttpRequest, timeout_ms: f64) -> Result<HttpResponse, HttpError> {\n\
         \x20   if timeout_ms <= 0.0 {\n\
         \x20       return http_send(options, request).await;\n\
         \x20   }\n\
         \x20   let deadline = std::time::Duration::from_secs_f64(timeout_ms / 1000.0);\n\
         \x20   match tokio::time::timeout(deadline, http_send(options, request)).await {\n\
         \x20       Ok(result) => result,\n\
         \x20       Err(_elapsed) => Err(format!(\"attempt timed out after {timeout_ms}ms\").into()),\n\
         \x20   }\n\
         }",
        vec![
            support("ClientOptions"),
            support("HttpRequest"),
            support("HttpResponse"),
            support("HttpError"),
        ],
    ));
    decls.push(Decl::raw_providing(
        "SleepFn",
        "/// The sleep behind the retry loop's backoff, held as a swappable\n\
         /// `pub(crate)` field on the generated client rather than called\n\
         /// directly: the parity harness (a `#[cfg(test)]` module of the same\n\
         /// crate) pins it to record delays deterministically. Milliseconds,\n\
         /// matching the backoff math every target shares.\n\
         pub type SleepFn = std::sync::Arc<dyn Fn(f64) -> BoxFuture<'static, ()> + Send + Sync>;",
        vec![support("BoxFuture")],
    ));
    decls
}

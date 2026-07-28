//! The public runtime surface: the canonical request/response shapes, the
//! transport and hook seams, the raw [`Outcome`], and [`Runtime::execute`] —
//! the retry loop that interprets a [`WireDescriptor`] against one configured
//! transport.
//!
//! Cancellation is Rust's native one: `execute` holds no resource across an
//! `.await` point that outlives the future itself, so wrapping the call in
//! `tokio::time::timeout` or racing it in `tokio::select!` and dropping it
//! cancels cleanly at whichever await point it was suspended on (build,
//! per-attempt timeout, the transport call, or a backoff sleep) — there is no
//! separate cancellation token to thread through, unlike Go's `context.Context`
//! or TypeScript's `AbortSignal`, which exist because their languages have no
//! drop-to-cancel primitive of their own.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::descriptor::WireDescriptor;
use crate::request::{
    build_body, build_headers, build_path, build_query, has_header, resolve_endpoint,
};
use crate::retry::{backoff_delay, is_retryable, resolve_max_retries, resolve_timeout};

/// A boxed, `Send` future, the shared shape every async seam in this crate
/// returns (transport calls, hooks, the sleep seam).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The error channel reserved for a hook failure: it propagates raw for the
/// generated wrapper to classify, and is never misreported as a transport
/// failure nor retried.
pub type ExecuteError = Box<dyn std::error::Error + Send + Sync>;

/// The request the runtime builds from a descriptor before sending it. A
/// `before_request` hook receives it and may return a mutated copy (set an
/// auth header, sign the body); a canonical [`Transport`] receives it as the
/// whole call input. `body` is `None` when the request carries no body.
#[derive(Debug, Clone)]
pub struct CanonicalRequest {
    pub method: String,
    pub url: String,
    pub headers: HashMap<String, String>,
    pub body: Option<Vec<u8>>,
}

/// The response the runtime reads before classifying it. Header keys are
/// lowercased (HTTP header names are case-insensitive). An `after_response`
/// hook may return a mutated copy; a canonical [`Transport`] produces one as
/// the whole call output.
#[derive(Debug, Clone)]
pub struct CanonicalResponse {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// The canonical transport slot: adapt any HTTP stack by mapping
/// [`CanonicalRequest`]/[`CanonicalResponse`], without emulating
/// `reqwest::Client` directly.
///
/// Contract: one call is one attempt. The runtime owns retry (driven by the
/// descriptor's retry declaration), so a transport with internal retries does
/// not combine with it: either disable the transport's retries or do not
/// declare retry on the operations.
pub type Transport = Arc<
    dyn Fn(CanonicalRequest) -> BoxFuture<'static, Result<CanonicalResponse, ExecuteError>>
        + Send
        + Sync,
>;

/// A `before_request`/`after_response` lifecycle hook.
pub type BeforeRequestHook = Arc<
    dyn Fn(CanonicalRequest) -> BoxFuture<'static, Result<CanonicalRequest, ExecuteError>>
        + Send
        + Sync,
>;
pub type AfterResponseHook = Arc<
    dyn Fn(CanonicalResponse) -> BoxFuture<'static, Result<CanonicalResponse, ExecuteError>>
        + Send
        + Sync,
>;

/// The lifecycle slots the runtime invokes around the transport, once per
/// attempt. A hook error propagates raw (the generated wrapper owns turning
/// it into its taxonomy); it is never misreported as a transport failure and
/// is never retried.
#[derive(Default, Clone)]
pub struct Hooks {
    pub before_request: Option<BeforeRequestHook>,
    pub after_response: Option<AfterResponseHook>,
}

/// What the caller supplies once, shared across every operation. Exactly one
/// transport slot may be set: `client` (native `reqwest::Client`) or
/// `transport` (canonical); setting both is a construction error. Neither set
/// defaults to a fresh `reqwest::Client`. The runtime ships no auth of its
/// own.
#[derive(Default)]
pub struct Options {
    pub base_url: String,
    pub client: Option<reqwest::Client>,
    pub transport: Option<Transport>,
    pub headers: HashMap<String, String>,
    /// The resolved client fields the descriptor's ref positions (retry max,
    /// timeout, endpoint, declared headers) look up by canonical name. The
    /// runtime stays blind to where they came from.
    pub values: Map<String, Value>,
}

/// Discriminates the three raw results of a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutcomeKind {
    Success,
    Error,
    Transport,
}

/// The raw result of one call, deliberately free of any error taxonomy: the
/// generated SDK maps each kind onto its own idiomatic form, so the runtime
/// stays a thin, language-neutral transport. `status`/`body` are set for
/// success/error outcomes; `cause` for transport outcomes.
#[derive(Debug)]
pub struct Outcome {
    pub kind: OutcomeKind,
    pub status: u16,
    pub body: Option<String>,
    pub cause: Option<ExecuteError>,
}

/// `Options::client` and `Options::transport` were both set; the caller must
/// pick the native slot or the canonical one.
#[derive(Debug)]
pub struct ExclusiveTransportError;

impl std::fmt::Display for ExclusiveTransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Options.client and Options.transport are mutually exclusive: set the native slot or the canonical slot, not both"
        )
    }
}

impl std::error::Error for ExclusiveTransportError {}

/// The per-attempt deadline elapsed before the transport call finished.
#[derive(Debug)]
struct TimeoutError;

impl std::fmt::Display for TimeoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "attempt timed out")
    }
}

impl std::error::Error for TimeoutError {}

type SleepFn = Arc<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>;
type RandomFn = Arc<dyn Fn() -> f64 + Send + Sync>;

/// A "random-enough" `[0, 1)` value for backoff jitter, drawn from the
/// standard library's per-process random seed (no `rand` dependency; jitter
/// has no security requirement, and tests always inject a pinned seam).
fn default_random() -> f64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let seed = RandomState::new().build_hasher().finish();
    (seed as f64 / u64::MAX as f64).min(0.999_999_999)
}

/// Executes wire descriptors against one configured transport.
pub struct Runtime {
    options: Options,
    sleep: SleepFn,
    random: RandomFn,
}

impl Runtime {
    /// Validates the options and builds a Runtime. Setting both transport
    /// slots is an error: the caller must pick the native slot or the
    /// canonical one.
    pub fn new(options: Options) -> Result<Runtime, ExclusiveTransportError> {
        if options.client.is_some() && options.transport.is_some() {
            return Err(ExclusiveTransportError);
        }
        Ok(Runtime {
            options,
            sleep: Arc::new(|d| Box::pin(tokio::time::sleep(d))),
            random: Arc::new(default_random),
        })
    }

    /// A Runtime with the clock and jitter seams pinned, for deterministic
    /// retry-timing tests.
    #[cfg(test)]
    pub(crate) fn with_seams(options: Options, sleep: SleepFn, random: RandomFn) -> Runtime {
        Runtime {
            options,
            sleep,
            random,
        }
    }

    /// Interprets one wire descriptor against the configured transport and
    /// returns the raw [`Outcome`]. The returned error is reserved for a hook
    /// failure: it propagates raw for the generated wrapper to classify, and
    /// is never misreported as a transport failure nor retried.
    ///
    /// When the descriptor declares retry, an attempt whose outcome is
    /// retryable (a transport failure, or an error response matching a
    /// declared retryable error) is repeated up to the declared maximum, with
    /// exponential full-jitter backoff between attempts. The declared timeout
    /// bounds each attempt separately.
    pub async fn execute(
        &self,
        d: &WireDescriptor,
        input: &Value,
        hooks: Option<&Hooks>,
    ) -> Result<Outcome, ExecuteError> {
        let record = input.as_object().cloned().unwrap_or_default();
        let max_retries = resolve_max_retries(d.retry.as_ref(), &self.options.values);
        let timeout = resolve_timeout(d.timeout.as_ref(), &self.options.values);
        let mut attempt_no = 0u32;
        loop {
            let outcome = self.attempt(d, &record, hooks, timeout).await?;
            if attempt_no >= max_retries || !is_retryable(d, &outcome) {
                return Ok(outcome);
            }
            (self.sleep)(backoff_delay(attempt_no, (self.random)())).await;
            attempt_no += 1;
        }
    }

    async fn attempt(
        &self,
        d: &WireDescriptor,
        record: &Map<String, Value>,
        hooks: Option<&Hooks>,
        timeout: Duration,
    ) -> Result<Outcome, ExecuteError> {
        let body = build_body(d, record);
        let mut headers = build_headers(d, record, &self.options.headers, &self.options.values);
        if body.is_some() && !has_header(&headers, "content-type") {
            headers.insert("content-type".to_string(), "application/json".to_string());
        }
        let base = resolve_endpoint(&d.endpoint, &self.options.values)
            .unwrap_or_else(|| self.options.base_url.clone());
        let mut url = format!("{base}{}", build_path(d, record, &self.options.values));
        let qs = build_query(d, record);
        if !qs.is_empty() {
            url.push('?');
            url.push_str(&qs);
        }

        let mut request = CanonicalRequest {
            method: d.http_method.clone(),
            url,
            headers,
            body,
        };
        if let Some(hook) = hooks.and_then(|h| h.before_request.as_ref()) {
            request = hook(request).await?;
        }

        // The timeout covers the transport call only, starting after the
        // before_request hook so a slow hook does not eat into the attempt.
        let call = self.call_transport(request);
        let outcome = if timeout.is_zero() {
            call.await
        } else {
            match tokio::time::timeout(timeout, call).await {
                Ok(result) => result,
                Err(_elapsed) => Err(Box::new(TimeoutError) as ExecuteError),
            }
        };
        let response = match outcome {
            Ok(response) => response,
            Err(cause) => {
                return Ok(Outcome {
                    kind: OutcomeKind::Transport,
                    status: 0,
                    body: None,
                    cause: Some(cause),
                })
            }
        };

        let response = if let Some(hook) = hooks.and_then(|h| h.after_response.as_ref()) {
            hook(response).await?
        } else {
            response
        };

        if is_success_status(d, response.status) {
            Ok(Outcome {
                kind: OutcomeKind::Success,
                status: response.status,
                body: Some(apply_response_bindings(d, &response)),
                cause: None,
            })
        } else {
            Ok(Outcome {
                kind: OutcomeKind::Error,
                status: response.status,
                body: Some(response.body),
                cause: None,
            })
        }
    }

    async fn call_transport(
        &self,
        req: CanonicalRequest,
    ) -> Result<CanonicalResponse, ExecuteError> {
        if let Some(transport) = &self.options.transport {
            return transport(req).await;
        }
        let client = self.options.client.clone().unwrap_or_default();
        let mut builder = client.request(req.method.parse::<reqwest::Method>()?, &req.url);
        for (name, value) in &req.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = req.body {
            builder = builder.body(body);
        }
        let response = builder.send().await?;
        let status = response.status().as_u16();
        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_lowercase(),
                    v.to_str().unwrap_or_default().to_string(),
                )
            })
            .collect();
        let body = response.text().await?;
        Ok(CanonicalResponse {
            status,
            headers,
            body,
        })
    }
}

/// A 2xx is a success even when its exact code was not the one declared (a
/// server may answer 200 where 201 was declared); a declared success code
/// outside the 2xx range still counts.
fn is_success_status(d: &WireDescriptor, status: u16) -> bool {
    if (200..300).contains(&status) {
        return true;
    }
    d.success.iter().any(|s| s.status == status as i64)
}

/// Reads the response-bound members off the (canonical) response (a header
/// value, or the status code) and folds them into the decoded body so the
/// generated decoder sees them as ordinary fields. Applied only on success;
/// interpreting the descriptor is the runtime's job, which keeps the
/// generated client blind.
fn apply_response_bindings(d: &WireDescriptor, res: &CanonicalResponse) -> String {
    if d.response_bindings.is_empty() {
        return res.body.clone();
    }
    // A non-object or empty body leaves the response-bound fields to stand on
    // their own.
    let mut object: Map<String, Value> = serde_json::from_str(&res.body).unwrap_or_default();
    for b in &d.response_bindings {
        if b.part.kind == "statusCode" {
            object.insert(b.member.clone(), Value::from(res.status));
            continue;
        }
        // Header names are case-insensitive; response header keys are lowercased.
        let value = res
            .headers
            .get(&b.part.name.to_lowercase())
            .map(|v| Value::String(v.clone()))
            .unwrap_or(Value::Null);
        object.insert(b.member.clone(), value);
    }
    // Every value in the object came off decoded JSON, an int status, or a
    // header string, so re-marshalling cannot fail.
    serde_json::to_string(&object).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn descriptor(json_text: &str) -> WireDescriptor {
        WireDescriptor::parse(json_text).unwrap()
    }

    /// A transport that answers each call from `responses` in order and
    /// panics (fails the test fast, instead of hanging or silently retrying
    /// forever) if called more times than scripted.
    fn scripted_transport(
        mut responses: Vec<Result<CanonicalResponse, &'static str>>,
    ) -> Transport {
        responses.reverse();
        let responses = Arc::new(Mutex::new(responses));
        Arc::new(move |_req: CanonicalRequest| {
            let responses = responses.clone();
            Box::pin(async move {
                match responses.lock().unwrap().pop() {
                    Some(Ok(res)) => Ok(res),
                    Some(Err(msg)) => Err(msg.into()),
                    None => panic!("transport called more times than scripted"),
                }
            })
        })
    }

    /// A transport that records every request it receives (for assembly
    /// assertions) and always answers with `response`.
    fn recording_transport(
        response: Result<CanonicalResponse, &'static str>,
    ) -> (Transport, Arc<Mutex<Vec<CanonicalRequest>>>) {
        let recorded: Arc<Mutex<Vec<CanonicalRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let for_closure = recorded.clone();
        let transport: Transport = Arc::new(move |req: CanonicalRequest| {
            let recorded = for_closure.clone();
            let response = response.clone();
            Box::pin(async move {
                recorded.lock().unwrap().push(req);
                response.map_err(Into::into)
            })
        });
        (transport, recorded)
    }

    fn ok(status: u16, body: &str) -> Result<CanonicalResponse, &'static str> {
        Ok(CanonicalResponse {
            status,
            headers: HashMap::new(),
            body: body.to_string(),
        })
    }

    fn deterministic_runtime(options: Options) -> Runtime {
        Runtime::with_seams(options, Arc::new(|_d| Box::pin(async {})), Arc::new(|| 0.5))
    }

    #[tokio::test]
    async fn a_success_status_is_never_retried() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[],"retry":{"max":{"lit":3}}}"#,
        );
        let options = Options {
            transport: Some(scripted_transport(vec![ok(200, "{}")])),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let outcome = runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(outcome.kind, OutcomeKind::Success);
        assert_eq!(outcome.status, 200);
    }

    #[tokio::test]
    async fn a_transport_failure_retries_until_success() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[],"retry":{"max":{"lit":2}}}"#,
        );
        let options = Options {
            transport: Some(scripted_transport(vec![
                Err("boom"),
                Err("boom"),
                ok(200, "{\"ok\":true}"),
            ])),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let outcome = runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(outcome.kind, OutcomeKind::Success);
        assert_eq!(outcome.body.as_deref(), Some("{\"ok\":true}"));
    }

    #[tokio::test]
    async fn exhausted_retries_return_the_last_outcome_after_exactly_max_plus_one_attempts() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[],"retry":{"max":{"lit":1}}}"#,
        );
        // Exactly two scripted failures: one attempt plus one retry. A broken
        // exit condition (the attempt counter never advancing, or the OR
        // becoming an AND) would call the transport a third time and panic.
        let options = Options {
            transport: Some(scripted_transport(vec![Err("boom"), Err("boom")])),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let outcome = runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(outcome.kind, OutcomeKind::Transport);
    }

    #[tokio::test]
    async fn no_retry_declared_means_one_attempt_ever() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let options = Options {
            transport: Some(scripted_transport(vec![ok(500, "{}")])),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let outcome = runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(outcome.kind, OutcomeKind::Error);
        assert_eq!(outcome.status, 500);
    }

    #[tokio::test]
    async fn a_hook_error_propagates_raw_and_is_never_retried() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[],"retry":{"max":{"lit":3}}}"#,
        );
        let options = Options {
            transport: Some(scripted_transport(vec![ok(200, "{}")])),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let hooks = Hooks {
            before_request: Some(Arc::new(|_req| {
                Box::pin(async { Err("hook failed".into()) })
            })),
            after_response: None,
        };
        let err = runtime
            .execute(&d, &Value::Object(Map::new()), Some(&hooks))
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "hook failed");
    }

    #[tokio::test]
    async fn a_timed_out_attempt_is_a_transport_failure() {
        // No retry is declared, so exactly one attempt is allowed. A bound on
        // the call count turns a broken retry-exit condition into a fast
        // panic instead of a transport that hangs the whole per-mutant test
        // run past its budget.
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[],"timeout":{"lit":10}}"#,
        );
        let calls = Arc::new(Mutex::new(0u32));
        let options = Options {
            transport: Some(Arc::new(move |_req| {
                let calls = calls.clone();
                Box::pin(async move {
                    {
                        let mut n = calls.lock().unwrap();
                        *n += 1;
                        assert!(
                            *n <= 1,
                            "transport called more than once with no retry declared"
                        );
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Ok(CanonicalResponse {
                        status: 200,
                        headers: HashMap::new(),
                        body: "{}".into(),
                    })
                })
            })),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let outcome = runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(outcome.kind, OutcomeKind::Transport);
    }

    #[test]
    fn setting_both_transport_slots_is_a_construction_error() {
        let options = Options {
            client: Some(reqwest::Client::new()),
            transport: Some(Arc::new(|_req| Box::pin(async { unreachable!() }))),
            ..Options::default()
        };
        assert!(Runtime::new(options).is_err());
    }

    #[test]
    fn is_success_status_accepts_any_2xx_and_a_declared_non_2xx() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[201,null]],"errors":[]}"#,
        );
        assert!(is_success_status(&d, 200));
        assert!(is_success_status(&d, 201));
        assert!(!is_success_status(&d, 400));
    }

    #[test]
    fn apply_response_bindings_folds_status_and_header_into_the_body() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[["code",{"kind":"statusCode"}],["req_id",{"kind":"header","name":"X-Request-Id"}]],"success":[[200,null]],"errors":[]}"#,
        );
        let res = CanonicalResponse {
            status: 200,
            headers: [("x-request-id".to_string(), "abc".to_string())].into(),
            body: "{\"ok\":true}".to_string(),
        };
        let out = apply_response_bindings(&d, &res);
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["ok"], Value::Bool(true));
        assert_eq!(parsed["code"], Value::from(200));
        assert_eq!(parsed["req_id"], Value::String("abc".into()));
    }

    #[tokio::test]
    async fn a_body_gets_a_default_content_type_when_none_is_set() {
        let d = descriptor(
            r#"{"http_method":"POST","uri":"/x","bindings":[["note",{"kind":"body","name":"note"}]],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let (transport, recorded) = recording_transport(ok(200, "{}"));
        let options = Options {
            transport: Some(transport),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let mut input = Map::new();
        input.insert("note".to_string(), Value::String("hi".into()));
        runtime
            .execute(&d, &Value::Object(input), None)
            .await
            .unwrap();
        let recorded = recorded.lock().unwrap();
        assert_eq!(
            recorded[0].headers.get("content-type"),
            Some(&"application/json".to_string())
        );
    }

    #[tokio::test]
    async fn a_caller_supplied_content_type_is_not_overridden() {
        let d = descriptor(
            r#"{"http_method":"POST","uri":"/x","bindings":[["note",{"kind":"body","name":"note"}]],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let (transport, recorded) = recording_transport(ok(200, "{}"));
        let options = Options {
            transport: Some(transport),
            headers: [("content-type".to_string(), "text/plain".to_string())].into(),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let mut input = Map::new();
        input.insert("note".to_string(), Value::String("hi".into()));
        runtime
            .execute(&d, &Value::Object(input), None)
            .await
            .unwrap();
        let recorded = recorded.lock().unwrap();
        assert_eq!(
            recorded[0].headers.get("content-type"),
            Some(&"text/plain".to_string())
        );
    }

    #[tokio::test]
    async fn a_bodyless_request_gets_no_content_type() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let (transport, recorded) = recording_transport(ok(200, "{}"));
        let options = Options {
            transport: Some(transport),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(
            recorded.lock().unwrap()[0].headers.get("content-type"),
            None
        );
    }

    #[tokio::test]
    async fn a_url_with_no_query_bindings_carries_no_question_mark() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let (transport, recorded) = recording_transport(ok(200, "{}"));
        let options = Options {
            base_url: "https://api.test".to_string(),
            transport: Some(transport),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        runtime
            .execute(&d, &Value::Object(Map::new()), None)
            .await
            .unwrap();
        assert_eq!(recorded.lock().unwrap()[0].url, "https://api.test/x");
    }

    #[tokio::test]
    async fn a_url_with_a_query_binding_appends_the_question_mark() {
        let d = descriptor(
            r#"{"http_method":"GET","uri":"/x","bindings":[["q",{"kind":"query","name":"q"}]],"response_bindings":[],"success":[[200,null]],"errors":[]}"#,
        );
        let (transport, recorded) = recording_transport(ok(200, "{}"));
        let options = Options {
            base_url: "https://api.test".to_string(),
            transport: Some(transport),
            ..Options::default()
        };
        let runtime = deterministic_runtime(options);
        let mut input = Map::new();
        input.insert("q".to_string(), Value::String("hi".into()));
        runtime
            .execute(&d, &Value::Object(input), None)
            .await
            .unwrap();
        assert_eq!(recorded.lock().unwrap()[0].url, "https://api.test/x?q=hi");
    }

    #[test]
    fn exclusive_transport_error_names_both_slots() {
        let msg = ExclusiveTransportError.to_string();
        assert!(msg.contains("Options.client"));
        assert!(msg.contains("Options.transport"));
    }

    #[test]
    fn timeout_error_message_names_the_timeout() {
        assert_eq!(TimeoutError.to_string(), "attempt timed out");
    }

    #[test]
    fn default_random_is_bounded_and_not_constant_across_calls() {
        let samples: Vec<f64> = (0..64).map(|_| default_random()).collect();
        assert!(samples.iter().all(|v| (0.0..1.0).contains(v)));
        assert!(samples.windows(2).any(|w| w[0] != w[1]));
    }
}

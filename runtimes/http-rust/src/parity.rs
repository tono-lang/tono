//! The cross-runtime parity suite: the same behavior vectors run against
//! every HTTP runtime, so retry/timeout/error handling cannot drift between
//! them. Jitter is pinned to 0.5 and backoff sleeps are recorded, matching
//! the Go and TypeScript runtimes' own harnesses.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::descriptor::WireDescriptor;
use crate::runtime::{
    CanonicalRequest, CanonicalResponse, Options, OutcomeKind, Runtime, Transport,
};

#[derive(Deserialize)]
struct ParityFile {
    vectors: Vec<ParityVector>,
}

#[derive(Deserialize)]
struct ParityVector {
    name: String,
    #[serde(default)]
    config: ParityConfig,
    // [status, code|null, retryable] triples: a stand-in for what a generated
    // decode<Op>Error + retryable() pair would classify, without depending on
    // any generated code to exist.
    #[serde(default)]
    declared_errors: Vec<(u16, Option<String>, bool)>,
    #[serde(default)]
    input: Map<String, Value>,
    script: Vec<ParityStep>,
    expect: ParityExpect,
}

/// Real client construction for the TypeScript harness (which drives a
/// generated SDK directly) and, here, the literal retry/timeout values a
/// synthetic descriptor is built from.
#[derive(Deserialize, Default)]
struct ParityConfig {
    max_retries: Option<f64>,
    timeout_ms: Option<f64>,
}

/// Rebuilds the synthetic `WireDescriptor` a vector's config describes: a
/// retry key only when the vector sets `max_retries`, a timeout key only
/// when it sets `timeout_ms`. Which fields are set already says everything
/// spec.tono's op declares (a `@retry` field or a `@timeout` field), so this
/// needs no per-op knowledge and nothing here changes when an op is added,
/// renamed, or removed there. `Runtime`/`execute` are exercised exactly as
/// before; only the descriptor's origin moves from "parsed off JSON" to
/// "built from config," since the JSON vector no longer carries one
/// directly.
fn descriptor_for(config: &ParityConfig) -> Value {
    let mut base = serde_json::json!({
        "http_method": "POST",
        "uri": "/x",
        "bindings": [],
        "response_bindings": [],
        "success": [[200, null]],
    });
    let obj = base.as_object_mut().unwrap();
    if let Some(max_retries) = config.max_retries {
        obj.insert(
            "retry".into(),
            serde_json::json!({ "max": { "lit": max_retries } }),
        );
    }
    if let Some(timeout_ms) = config.timeout_ms {
        obj.insert("timeout".into(), serde_json::json!({ "lit": timeout_ms }));
    }
    base
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ParityStep {
    Response { status: u16, body: String },
    TransportFailure,
    Hang,
}

#[derive(Deserialize)]
struct ParityExpect {
    outcome: String,
    #[serde(default)]
    status: u16,
    #[serde(default)]
    body: String,
    attempts: u32,
    delays_ms: Vec<f64>,
    #[serde(default)]
    request: Option<ParityRequest>,
}

#[derive(Deserialize)]
struct ParityRequest {
    #[serde(default)]
    url: String,
    #[serde(default)]
    headers: HashMap<String, String>,
}

/// A transport that answers each attempt from the vector's script in order,
/// failing the test outright if called more times than the script has steps.
fn scripted_transport(
    script: Vec<ParityStep>,
    attempts: Arc<Mutex<u32>>,
    requests: Arc<Mutex<Vec<CanonicalRequest>>>,
) -> Transport {
    let script = Arc::new(script);
    Arc::new(move |req: CanonicalRequest| {
        let script = script.clone();
        let attempts = attempts.clone();
        let requests = requests.clone();
        Box::pin(async move {
            let index = {
                let mut a = attempts.lock().unwrap();
                let i = *a as usize;
                assert!(
                    i < script.len(),
                    "transport called {} times for a {}-step script",
                    i + 1,
                    script.len()
                );
                *a += 1;
                i
            };
            requests.lock().unwrap().push(req);
            match &script[index] {
                ParityStep::Response { status, body } => Ok(CanonicalResponse {
                    status: *status,
                    headers: HashMap::new(),
                    body: body.clone(),
                }),
                ParityStep::TransportFailure => Err("scripted transport failure".into()),
                ParityStep::Hang => {
                    // Honors the per-attempt timeout, as the canonical transport
                    // contract asks: `tokio::time::timeout` (in `attempt`) drops
                    // this future once the real deadline fires, well before the
                    // 2s bail-out below. The bail-out only guards against a
                    // mutant that breaks the deadline, so it fails the vector
                    // fast instead of hanging the whole test binary.
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    Err("hang step: the per-attempt deadline never fired".into())
                }
            }
        })
    })
}

fn outcome_name(kind: OutcomeKind) -> &'static str {
    match kind {
        OutcomeKind::Success => "success",
        OutcomeKind::Error => "error",
        OutcomeKind::Transport => "transport",
    }
}

/// Reads the shared vectors file. `TONO_PARITY_VECTORS` overrides the
/// location and makes absence a failure; without it, a missing file skips
/// (a mutation-testing sandbox may copy only this crate).
fn load_parity_vectors() -> Option<Vec<ParityVector>> {
    let path = std::env::var("TONO_PARITY_VECTORS").ok();
    let required = path.is_some();
    let path = path.unwrap_or_else(|| "../parity/vectors.json".to_string());
    let data = match std::fs::read_to_string(&path) {
        Ok(data) => data,
        Err(e) => {
            assert!(!required, "TONO_PARITY_VECTORS set but unreadable: {e}");
            eprintln!("parity vectors not present at {path}: {e}, skipping");
            return None;
        }
    };
    let file: ParityFile = serde_json::from_str(&data).expect("parity vectors");
    assert!(
        !file.vectors.is_empty(),
        "parity vectors file has no vectors"
    );
    Some(file.vectors)
}

#[tokio::test]
async fn parity_vectors() {
    let Some(vectors) = load_parity_vectors() else {
        return;
    };

    for vector in vectors {
        let descriptor = descriptor_for(&vector.config);
        let descriptor_text = serde_json::to_string(&descriptor).unwrap();
        let d = WireDescriptor::parse(&descriptor_text)
            .unwrap_or_else(|e| panic!("{}: descriptor: {e}", vector.name));

        let attempts = Arc::new(Mutex::new(0u32));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let delays: Arc<Mutex<Vec<Duration>>> = Arc::new(Mutex::new(Vec::new()));

        let options = Options {
            base_url: "https://api.test".to_string(),
            transport: Some(scripted_transport(
                vector.script,
                attempts.clone(),
                requests.clone(),
            )),
            ..Options::default()
        };
        let delays_for_seam = delays.clone();
        let runtime = Runtime::with_seams(
            options,
            Arc::new(move |d| {
                let delays = delays_for_seam.clone();
                Box::pin(async move {
                    delays.lock().unwrap().push(d);
                })
            }),
            Arc::new(|| 0.5),
        );

        // Stands in for a generated decode<Op>Error + retryable() pair: the
        // first status match with an agreeing code decides (a declared code
        // must equal the body's "code" field; a null code matches any body).
        let declared_errors = vector.declared_errors;
        let retryable = move |status: u16, body: &str| -> bool {
            let code = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str().map(str::to_string)));
            for (declared_status, declared_code, is_retryable) in &declared_errors {
                if *declared_status != status {
                    continue;
                }
                let agrees = match declared_code {
                    None => true,
                    Some(c) => code.as_deref() == Some(c.as_str()),
                };
                if agrees {
                    return *is_retryable;
                }
            }
            false
        };

        let outcome = runtime
            .execute(&d, &Value::Object(vector.input), None, Some(&retryable))
            .await
            .unwrap_or_else(|e| panic!("{}: execute: {e}", vector.name));

        assert_eq!(
            outcome_name(outcome.kind),
            vector.expect.outcome,
            "{}: outcome",
            vector.name
        );
        if outcome.kind == OutcomeKind::Transport {
            assert!(
                outcome.cause.is_some(),
                "{}: transport outcome without a cause",
                vector.name
            );
        } else {
            assert_eq!(
                outcome.status, vector.expect.status,
                "{}: status",
                vector.name
            );
            assert_eq!(
                outcome.body.as_deref().unwrap_or(""),
                vector.expect.body,
                "{}: body",
                vector.name
            );
        }

        let attempts_count = *attempts.lock().unwrap();
        assert_eq!(
            attempts_count, vector.expect.attempts,
            "{}: attempts",
            vector.name
        );

        let recorded_delays = delays.lock().unwrap();
        assert_eq!(
            recorded_delays.len(),
            vector.expect.delays_ms.len(),
            "{}: delays",
            vector.name
        );
        for (i, ms) in vector.expect.delays_ms.iter().enumerate() {
            let want = Duration::from_secs_f64(ms / 1000.0);
            assert_eq!(recorded_delays[i], want, "{}: delay {i}", vector.name);
        }

        if let Some(want) = &vector.expect.request {
            let recorded = requests.lock().unwrap();
            let first = recorded.first().unwrap_or_else(|| {
                panic!(
                    "{}: request expectation with no recorded request",
                    vector.name
                )
            });
            if !want.url.is_empty() {
                assert_eq!(first.url, want.url, "{}: request url", vector.name);
            }
            for (name, value) in &want.headers {
                assert_eq!(
                    first.headers.get(name),
                    Some(value),
                    "{}: request header {name}",
                    vector.name
                );
            }
        }
    }
}

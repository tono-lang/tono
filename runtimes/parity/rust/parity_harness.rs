// The cross-runtime parity suite, Rust side: drives the real generated SDK
// compiled from ../spec.tono. This file is not run in place:
// scripts/run-parity.sh copies it (and ../vectors.json) into the generated
// crate's src/ and appends `#[cfg(test)] mod parity_harness;` to the
// generated lib.rs, so it builds as a test module of the SDK crate itself.
// Same-crate visibility is the point: the retry backoff's sleep/random seam
// is a pub(crate) field pair on the generated client, and pinning it (jitter
// 0.5, an instant recording sleep) is what makes the delay expectations
// deterministic. The per-attempt @timeout still runs through a real
// tokio::time::timeout inside the generated transport; only the backoff is
// mocked, so a "hang" vector costs a few real milliseconds, not the seconds
// a naive backoff would otherwise take.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::Deserialize;

use crate::parity::{
    decode_retrying_error, APIError, APIFailure, Client, Duration, HttpRequest, HttpResponse,
    HttpTransport, Thing, TonoError,
};

const VECTORS: &str = include_str!("vectors.json");

#[derive(Deserialize)]
struct ParityFile {
    vectors: Vec<ParityVector>,
}

// `input` and `declared_errors` (kept for Go's still-runtime-level harness)
// are ignored here: every parity op takes an empty `thing`, and the
// generated SDK decides retryability for real, from spec.tono's own
// @errorCode/@retryable declarations.
#[derive(Deserialize)]
struct ParityVector {
    name: String,
    op: String,
    #[serde(default)]
    config: ParityConfig,
    script: Vec<ParityStep>,
    expect: Expectation,
}

// Real client construction values, not descriptor refs: max_retries feeds
// the builder's with_max_retries, timeout_ms its with_timeout (as a
// millisecond duration literal).
#[derive(Deserialize, Default)]
struct ParityConfig {
    max_retries: Option<i32>,
    timeout_ms: Option<u64>,
}

#[derive(Deserialize)]
struct ParityStep {
    kind: String,
    status: Option<u16>,
    body: Option<String>,
}

#[derive(Deserialize)]
struct Expectation {
    outcome: String,
    status: Option<u16>,
    body: Option<String>,
    attempts: usize,
    delays_ms: Vec<f64>,
}

/// Scripts one canned answer per attempt, exactly like the other targets'
/// harnesses: `response` resolves, `transport_failure` fails the attempt,
/// `hang` sleeps out the per-attempt timeout (bounded at 2s, so a broken
/// timeout fails the expectations instead of wedging the run). A call past
/// the script's end fails the attempt loudly.
fn scripted(script: Vec<ParityStep>) -> (HttpTransport, Arc<Mutex<Vec<HttpRequest>>>) {
    let requests: Arc<Mutex<Vec<HttpRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = requests.clone();
    let script = Arc::new(script);
    let transport: HttpTransport = Arc::new(move |req: HttpRequest| {
        let recorded = recorded.clone();
        let script = script.clone();
        Box::pin(async move {
            let at = {
                let mut seen = recorded.lock().unwrap();
                seen.push(req);
                seen.len() - 1
            };
            let Some(step) = script.get(at) else {
                return Err(format!(
                    "transport called {} times for a {}-step script",
                    at + 1,
                    script.len()
                )
                .into());
            };
            match step.kind.as_str() {
                "response" => Ok(HttpResponse {
                    status: step.status.unwrap_or(0),
                    headers: HashMap::new(),
                    body: step.body.clone().unwrap_or_default(),
                }),
                "transport_failure" => Err("scripted transport failure".into()),
                "hang" => {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    Err("hang step: the per-attempt timeout never fired".into())
                }
                other => panic!("unknown script step kind {other}"),
            }
        })
    });
    (transport, requests)
}

/// The classification a raw (status, body) error pair should decode to:
/// `retrying` declares errors, so its expectation runs the SDK's own
/// discriminator (the same function the retry loop consulted); the other
/// ops declare none, so any error response is Undeclared.
fn expected_error(op: &str, status: u16, body: &str) -> TonoError {
    if op == "retrying" {
        decode_retrying_error(status, body)
    } else {
        TonoError::Api(APIFailure::Undeclared(APIError {
            status,
            body: body.to_string(),
        }))
    }
}

async fn run_vector(vector: ParityVector) {
    let name = vector.name.clone();
    let (transport, requests) = scripted(vector.script);
    let mut builder = Client::builder("https://api.test".to_string());
    if let Some(max) = vector.config.max_retries {
        builder = builder.with_max_retries(max);
    }
    if let Some(ms) = vector.config.timeout_ms {
        builder = builder.with_timeout(Duration(format!("{ms}ms")));
    }
    let mut client = builder
        .build_with_transport(Some(transport))
        .expect("construct client");

    let delays: Arc<Mutex<Vec<f64>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded_delays = delays.clone();
    client.sleep = Arc::new(move |ms| {
        recorded_delays.lock().unwrap().push(ms);
        Box::pin(async {})
    });
    client.random = Arc::new(|| 0.5);

    let result: Result<Thing, TonoError> = match vector.op.as_str() {
        "retrying" => client.retrying(Thing {}).await,
        "retrying_with_timeout" => client.retrying_with_timeout(Thing {}).await,
        "timeout_only" => client.timeout_only(Thing {}).await,
        other => panic!("{name}: unknown parity op {other}"),
    };

    match vector.expect.outcome.as_str() {
        // The decoded value re-serializes to the expected body; the expected
        // 200 status is implied by the success itself (the generated method
        // returns the typed value, not the raw response).
        "success" => match result {
            Ok(value) => assert_eq!(
                serde_json::to_string(&value).expect("serialize outcome"),
                vector.expect.body.clone().unwrap_or_default(),
                "{name}: success body"
            ),
            Err(e) => panic!("{name}: expected success, got {e:?}"),
        },
        "transport" => match result {
            Err(TonoError::Transport(_)) => {}
            other => panic!("{name}: expected a transport failure, got {other:?}"),
        },
        "error" => match result {
            Err(err) => {
                let status = vector.expect.status.expect("an error vector carries a status");
                let body = vector.expect.body.clone().unwrap_or_default();
                let expected = expected_error(&vector.op, status, &body);
                assert_eq!(
                    format!("{err:?}"),
                    format!("{expected:?}"),
                    "{name}: error classification"
                );
            }
            Ok(v) => panic!("{name}: expected an error, got {v:?}"),
        },
        other => panic!("{name}: unknown expected outcome {other}"),
    }
    assert_eq!(
        requests.lock().unwrap().len(),
        vector.expect.attempts,
        "{name}: attempts"
    );
    assert_eq!(
        *delays.lock().unwrap(),
        vector.expect.delays_ms,
        "{name}: delays"
    );
}

#[tokio::test]
async fn parity_vectors() {
    let file: ParityFile = serde_json::from_str(VECTORS).expect("parse vectors.json");
    assert!(!file.vectors.is_empty(), "vectors.json carries vectors");
    for vector in file.vectors {
        run_vector(vector).await;
    }
}

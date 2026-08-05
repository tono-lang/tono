//! Backoff and retry classification: the retry-loop math shared verbatim with
//! the Go and TypeScript runtimes, pinned by `../parity/vectors.json`.

use std::time::Duration;

use serde_json::{Map, Value};

use crate::descriptor::{RetrySpec, ValueSource};
use crate::runtime::{Outcome, OutcomeKind, RetryPredicate};

/// Backoff is a fixed runtime policy (exponential with full jitter), not
/// declarable in the descriptor: delay before retry `n` is
/// `random() * min(cap, base * 2^n)`. The constants are part of the
/// cross-runtime parity contract and must match the other HTTP runtimes.
const BACKOFF_BASE_MS: f64 = 100.0;
const BACKOFF_CAP_MS: f64 = 2000.0;

pub fn backoff_delay(retry: u32, random: f64) -> Duration {
    let exp = BACKOFF_CAP_MS.min(BACKOFF_BASE_MS * 2f64.powi(retry as i32));
    Duration::from_secs_f64(random * exp / 1000.0)
}

/// Reads a [`ValueSource`]: a literal yields itself; a ref yields the numeric
/// value under that name in the client values. Anything else (absent ref,
/// non-numeric value) yields `None`, which disables the feature rather than
/// failing the call: the descriptor and the resolved values are produced by
/// generated code, and the runtime stays blind to their provenance.
pub fn resolve_number(source: &ValueSource, values: &Map<String, Value>) -> Option<f64> {
    if let Some(lit) = source.lit {
        return Some(lit);
    }
    let name = source.r#ref.as_ref()?;
    match values.get(name) {
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    }
}

/// The maximum number of retries after the first attempt. No retry
/// declaration or a non-numeric value means zero retries; a fractional value
/// floors. A value below one also means zero: floored and cast to `u32`, it
/// saturates there on its own (no separate guard needed).
pub fn resolve_max_retries(spec: Option<&RetrySpec>, values: &Map<String, Value>) -> u32 {
    let Some(spec) = spec else { return 0 };
    match resolve_number(&spec.max, values) {
        Some(n) => n.floor() as u32,
        None => 0,
    }
}

/// The per-attempt deadline. Absent or non-numeric means no deadline. A
/// negative value is clamped to zero before conversion (`Duration` cannot
/// represent a negative span, and `from_secs_f64` panics on one); zero and
/// every positive value convert directly.
pub fn resolve_timeout(source: Option<&ValueSource>, values: &Map<String, Value>) -> Duration {
    let Some(source) = source else {
        return Duration::ZERO;
    };
    match resolve_number(source, values) {
        Some(ms) => Duration::from_secs_f64(ms.max(0.0) / 1000.0),
        None => Duration::ZERO,
    }
}

/// Classifies one outcome for the retry loop: a transport failure always
/// retries; a success never does; an error status retries only when the
/// caller-supplied predicate accepts its status and raw body. The predicate
/// is the generated client's own decode/retryable() pair, so `None` (an op
/// with no declared errors) means no error response is ever retryable.
pub fn is_retryable(outcome: &Outcome, retryable: Option<&RetryPredicate>) -> bool {
    match outcome.kind {
        OutcomeKind::Transport => true,
        OutcomeKind::Success => false,
        OutcomeKind::Error => retryable
            .map(|f| f(outcome.status, outcome.body.as_deref().unwrap_or("")))
            .unwrap_or(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_matches_the_parity_table() {
        assert_eq!(backoff_delay(0, 0.5), Duration::from_millis(50));
        assert_eq!(backoff_delay(1, 0.5), Duration::from_millis(100));
        assert_eq!(backoff_delay(2, 0.5), Duration::from_millis(200));
        assert_eq!(backoff_delay(5, 0.5), Duration::from_millis(1000));
        assert_eq!(backoff_delay(9, 1.0), Duration::from_millis(2000));
    }

    fn values(pairs: &[(&str, Value)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn resolve_number_prefers_a_literal_over_a_ref() {
        let source = ValueSource {
            r#ref: Some("x".into()),
            lit: Some(3.0),
        };
        assert_eq!(resolve_number(&source, &Map::new()), Some(3.0));
    }

    #[test]
    fn resolve_number_reads_a_ref_from_values() {
        let source = ValueSource {
            r#ref: Some("max_retries".into()),
            lit: None,
        };
        let values = values(&[("max_retries", Value::from(2))]);
        assert_eq!(resolve_number(&source, &values), Some(2.0));
    }

    #[test]
    fn resolve_number_is_none_for_a_missing_ref() {
        let source = ValueSource {
            r#ref: Some("absent".into()),
            lit: None,
        };
        assert_eq!(resolve_number(&source, &Map::new()), None);
    }

    #[test]
    fn resolve_max_retries_is_zero_with_no_spec() {
        assert_eq!(resolve_max_retries(None, &Map::new()), 0);
    }

    #[test]
    fn resolve_max_retries_floors_a_fractional_value() {
        let spec = RetrySpec {
            max: ValueSource {
                r#ref: None,
                lit: Some(2.9),
            },
        };
        assert_eq!(resolve_max_retries(Some(&spec), &Map::new()), 2);
    }

    #[test]
    fn resolve_max_retries_disables_below_one() {
        let spec = RetrySpec {
            max: ValueSource {
                r#ref: None,
                lit: Some(0.5),
            },
        };
        assert_eq!(resolve_max_retries(Some(&spec), &Map::new()), 0);
    }

    #[test]
    fn resolve_max_retries_disables_a_negative_value() {
        let spec = RetrySpec {
            max: ValueSource {
                r#ref: None,
                lit: Some(-4.0),
            },
        };
        assert_eq!(resolve_max_retries(Some(&spec), &Map::new()), 0);
    }

    #[test]
    fn resolve_max_retries_keeps_a_large_value_uncapped() {
        let spec = RetrySpec {
            max: ValueSource {
                r#ref: None,
                lit: Some(9.0),
            },
        };
        assert_eq!(resolve_max_retries(Some(&spec), &Map::new()), 9);
    }

    #[test]
    fn resolve_timeout_is_zero_when_absent() {
        assert_eq!(resolve_timeout(None, &Map::new()), Duration::ZERO);
    }

    #[test]
    fn resolve_timeout_reads_milliseconds() {
        let source = ValueSource {
            r#ref: None,
            lit: Some(30.0),
        };
        assert_eq!(
            resolve_timeout(Some(&source), &Map::new()),
            Duration::from_millis(30)
        );
    }

    #[test]
    fn resolve_timeout_clamps_a_negative_value_to_zero_instead_of_panicking() {
        let source = ValueSource {
            r#ref: None,
            lit: Some(-30.0),
        };
        assert_eq!(resolve_timeout(Some(&source), &Map::new()), Duration::ZERO);
    }

    #[test]
    fn resolve_timeout_of_exactly_zero_is_zero() {
        let source = ValueSource {
            r#ref: None,
            lit: Some(0.0),
        };
        assert_eq!(resolve_timeout(Some(&source), &Map::new()), Duration::ZERO);
    }

    fn error_outcome(status: u16, body: &str) -> Outcome {
        Outcome {
            kind: OutcomeKind::Error,
            status,
            body: Some(body.to_string()),
            cause: None,
        }
    }

    /// Stands in for a generated `decode_<op>_error` + `retryable()` pair:
    /// the first status match with an agreeing code decides (a declared code
    /// must equal the body's `"code"` field; a null code matches any body).
    fn classify_like_discriminator(
        declared: &'static [(u16, Option<&'static str>, bool)],
    ) -> impl Fn(u16, &str) -> bool {
        move |status, body| {
            let code = serde_json::from_str::<Value>(body)
                .ok()
                .and_then(|v| v.get("code").and_then(|c| c.as_str().map(str::to_string)));
            for &(declared_status, declared_code, retryable) in declared {
                if declared_status != status {
                    continue;
                }
                let agrees = match declared_code {
                    None => true,
                    Some(c) => code.as_deref() == Some(c),
                };
                if agrees {
                    return retryable;
                }
            }
            false
        }
    }

    #[test]
    fn a_transport_outcome_always_retries() {
        let outcome = Outcome {
            kind: OutcomeKind::Transport,
            status: 0,
            body: None,
            cause: None,
        };
        assert!(is_retryable(&outcome, None));
    }

    #[test]
    fn a_success_outcome_never_retries() {
        let outcome = Outcome {
            kind: OutcomeKind::Success,
            status: 200,
            body: Some("{}".into()),
            cause: None,
        };
        assert!(!is_retryable(&outcome, None));
    }

    #[test]
    fn an_absent_predicate_never_retries_an_error() {
        assert!(!is_retryable(&error_outcome(500, "{}"), None));
    }

    #[test]
    fn a_null_code_matches_any_body() {
        let retryable = classify_like_discriminator(&[(503, None, true)]);
        assert!(is_retryable(
            &error_outcome(503, "not json"),
            Some(&retryable)
        ));
    }

    #[test]
    fn a_mismatched_code_falls_through_to_no_match() {
        let retryable = classify_like_discriminator(&[(503, Some("unavailable"), true)]);
        assert!(!is_retryable(
            &error_outcome(503, "not json"),
            Some(&retryable)
        ));
    }

    #[test]
    fn the_first_status_and_code_match_decides() {
        let retryable = classify_like_discriminator(&[
            (429, Some("overloaded"), true),
            (429, Some("quota_exceeded"), false),
        ]);
        assert!(is_retryable(
            &error_outcome(429, r#"{"code":"overloaded"}"#),
            Some(&retryable)
        ));
        assert!(!is_retryable(
            &error_outcome(429, r#"{"code":"quota_exceeded"}"#),
            Some(&retryable)
        ));
    }
}

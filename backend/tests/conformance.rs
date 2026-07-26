//! Cross-language differential harness: every generated port decodes and
//! re-encodes the same shared batch of wire documents (the canonical fixture
//! plus seeded pseudo-random documents), and the re-encoded JSON of all present
//! ports must be canonically equal pairwise (parse to `Value`, then compare, so
//! key order is insignificant). Mutual agreement catches drift between ports
//! without electing a winner; the golden harness is what says which output is
//! right (RFC's N-versus-1 role).
//!
//! The random batch is deterministic: a fixed default seed keeps CI
//! reproducible, and `TONO_DIFFERENTIAL_SEED` explores other batches locally.
//! A language whose toolchain is absent is skipped; the test still asserts
//! agreement across whatever is present. Skipped under coverage.

use serde_json::{json, Value};

mod common;
use common::{ports, CANONICAL_WIRE as CANONICAL};

const DEFAULT_SEED: u64 = 0x746f_6e6f_2d64_6966; // "tono-dif" as bytes
const RANDOM_DOCS: usize = 32;

/// xorshift64*: small, seedable, and stable across platforms, which is all the
/// shared-input contract needs.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(if seed == 0 { DEFAULT_SEED } else { seed })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn token(&mut self, max_len: u64) -> String {
        const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789_";
        let len = 1 + self.below(max_len);
        (0..len)
            .map(|i| {
                let pool = if i == 0 { 26 } else { ALPHABET.len() as u64 };
                ALPHABET[self.below(pool) as usize] as char
            })
            .collect()
    }
}

/// Standard base64 with padding, matching what every port emits for bytes.
fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 0x3F] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// A pseudo-random wire document for the shared matrix module. Every axis of the
/// wire matrix varies: full-range i64 as string, random bytes as base64, the
/// optional field present or absent, known and unknown values for both open
/// enums, both union arms, and an `@entries` pairs array with unique keys (key
/// order matters on the wire, duplicate semantics do not).
fn random_wire_doc(rng: &mut Rng) -> Value {
    let mut doc = serde_json::Map::new();
    doc.insert("account_id".into(), json!((rng.next() as i64).to_string()));
    let secret: Vec<u8> = (0..rng.below(13)).map(|_| rng.next() as u8).collect();
    doc.insert("secret".into(), json!(base64(&secret)));
    if rng.chance(50) {
        doc.insert(
            "tip".into(),
            json!((rng.next() as i64 % 10_000).to_string()),
        );
    }
    let status = match rng.below(3) {
        0 => "active".to_string(),
        1 => "closed".to_string(),
        _ => rng.token(8),
    };
    doc.insert("status".into(), json!(status));
    let code = match rng.below(4) {
        0 => 200,
        1 => 404,
        2 => 500,
        _ => rng.below(100_000) as i64,
    };
    doc.insert("code".into(), json!(code));
    let method = if rng.chance(50) {
        json!({"type": "card", "last4": rng.token(6)})
    } else {
        json!({"type": "bank", "iban": rng.token(12)})
    };
    doc.insert("method".into(), method);
    let n_counts = rng.below(6) as usize;
    let mut keys: Vec<i32> = Vec::new();
    while keys.len() < n_counts {
        let k = rng.next() as i32;
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    let counts: Vec<Value> = keys.into_iter().map(|k| json!([k, rng.token(6)])).collect();
    doc.insert("counts".into(), json!(counts));
    Value::Object(doc)
}

#[test]
fn the_targets_agree_on_shared_random_input() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test conformance`");
        return;
    }
    let seed = std::env::var("TONO_DIFFERENTIAL_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(DEFAULT_SEED);
    eprintln!("differential seed: {seed:#x}");
    let mut rng = Rng::new(seed);

    let canonical: Value = serde_json::from_str(CANONICAL).expect("canonical fixture is json");
    let mut inputs = vec![canonical];
    inputs.extend((0..RANDOM_DOCS).map(|_| random_wire_doc(&mut rng)));

    let outputs = ports::all_port_outputs(&inputs, "conformance");
    let present: Vec<(&str, &Vec<Value>)> = outputs
        .iter()
        .filter_map(|(name, v)| v.as_ref().map(|v| (*name, v)))
        .collect();
    assert!(
        !present.is_empty(),
        "no language toolchain available to check conformance"
    );
    eprintln!(
        "differential checked across: {:?}",
        present.iter().map(|(n, _)| *n).collect::<Vec<_>>()
    );

    let (first_name, first_outputs) = present[0];
    for (name, port_outputs) in &present[1..] {
        for (i, (a, b)) in first_outputs.iter().zip(port_outputs.iter()).enumerate() {
            assert_eq!(
                a, b,
                "{first_name} and {name} disagree on document {i} (seed {seed:#x})\n\
                 input: {}",
                inputs[i]
            );
        }
    }
}

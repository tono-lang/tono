//! Golden N-versus-1 harness (the RFC's designated-reference role): the
//! committed vectors in `tests/golden/wire_vectors.json` carry, for each wire
//! input, the output the designated reference port produced. Every generated
//! port re-encodes the same inputs and must match the committed outputs, which
//! collapses "which of N is right?" into "all agree with the reference".
//!
//! Two checks run: the reference itself must still reproduce its committed
//! outputs (a mismatch means the reference drifted, and the vectors must be
//! regenerated deliberately), and every other available port must match the
//! reference's outputs. Regenerate with:
//!
//! ```text
//! TONO_UPDATE_GOLDEN_WIRE=1 cargo test --test golden_wire
//! ```
//!
//! The rewritten file then shows up as a reviewable diff, like any golden
//! change. A language whose toolchain is absent is skipped; the reference is
//! always checked. Skipped under coverage.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

mod common;
use common::ports;

#[derive(Deserialize, Serialize)]
struct VectorFile {
    reference: String,
    vectors: Vec<Vector>,
}

#[derive(Deserialize, Serialize)]
struct Vector {
    name: String,
    input: Value,
    expected: Option<Value>,
}

fn vectors_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/wire_vectors.json")
}

#[test]
fn every_port_matches_the_reference_vectors() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test golden_wire`");
        return;
    }
    let text = std::fs::read_to_string(vectors_path()).expect("read wire_vectors.json");
    let mut file: VectorFile = serde_json::from_str(&text).expect("wire_vectors.json is valid");
    assert_eq!(
        file.reference,
        ports::REFERENCE_PORT,
        "the vector file's reference and the harness's designated reference must agree"
    );

    let inputs: Vec<Value> = file.vectors.iter().map(|v| v.input.clone()).collect();
    let outputs = ports::all_port_outputs(&inputs, "golden");
    let reference_outputs = outputs
        .iter()
        .find(|(name, _)| *name == file.reference)
        .and_then(|(_, v)| v.as_ref())
        .expect("the reference port's toolchain must be available");

    if std::env::var_os("TONO_UPDATE_GOLDEN_WIRE").is_some() {
        for (vector, output) in file.vectors.iter_mut().zip(reference_outputs) {
            vector.expected = Some(output.clone());
        }
        let pretty = serde_json::to_string_pretty(&file).expect("encode wire_vectors.json");
        std::fs::write(vectors_path(), pretty + "\n").expect("write wire_vectors.json");
        eprintln!(
            "wire_vectors.json regenerated from the {} reference; review the diff",
            file.reference
        );
        return;
    }

    // The reference must reproduce its own committed vectors: a mismatch means
    // the reference drifted, and the vectors are only regenerated deliberately.
    for (vector, output) in file.vectors.iter().zip(reference_outputs) {
        let expected = vector.expected.as_ref().unwrap_or_else(|| {
            panic!(
                "vector {:?} has no committed output; regenerate with TONO_UPDATE_GOLDEN_WIRE=1",
                vector.name
            )
        });
        assert_eq!(
            output, expected,
            "the {} reference no longer reproduces vector {:?}; if the change is \
             intended, regenerate with TONO_UPDATE_GOLDEN_WIRE=1 and review the diff",
            file.reference, vector.name
        );
    }

    // N versus 1: every other available port must match the reference's vectors.
    let mut checked = vec![file.reference.as_str()];
    for (name, port_outputs) in &outputs {
        if *name == file.reference {
            continue;
        }
        let Some(port_outputs) = port_outputs else {
            eprintln!("golden: {name} toolchain absent, skipped");
            continue;
        };
        for (vector, output) in file.vectors.iter().zip(port_outputs) {
            assert_eq!(
                output,
                vector.expected.as_ref().unwrap(),
                "{name} disagrees with the {} reference on vector {:?}",
                file.reference,
                vector.name
            );
        }
        checked.push(name);
    }
    eprintln!("golden vectors checked across: {checked:?}");
}

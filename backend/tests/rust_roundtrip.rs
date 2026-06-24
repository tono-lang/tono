//! End-to-end check that the Rust the engine emits compiles and round-trips.
//!
//! Generates a module, formats it with rustfmt, writes it as the `models` module
//! of a small out-of-workspace crate, and runs that crate's driver with cargo.
//! The driver asserts the hard wire cases hold (i64 above 2^53 as a string, bytes
//! as base64, internally-tagged union, open-enum lenient decode, canonical
//! round-trip). Skips cleanly if the toolchain is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::render::render_file;
use tono_backend::codegen::targets::rust::emit::emit_module;
use tono_backend::codegen::targets::rust::types::rust_casing;
use tono_backend::codegen::targets::rust::RustRules;
use tono_backend::codegen::Formatter;

mod common;
use common::matrix_module as demo_module;

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/rust")
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn generated_rust_compiles_and_round_trips() {
    // Skip under coverage: this test shells out to a nested `cargo run`, which
    // would compile under inherited instrumentation. A dedicated CI job runs it
    // with a plain `cargo test`; the coverage job stays pure.
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test rust_roundtrip`");
        return;
    }
    if !have("rustfmt") || !have("cargo") {
        eprintln!("skipping: Rust toolchain (rustfmt/cargo) not available");
        return;
    }
    let dir = crate_dir();

    // Generate the module and format it with the engine's formatter (rustfmt).
    let file = emit_module(&demo_module(), &rust_casing());
    let formatter = Formatter::new("rustfmt", vec!["--edition".into(), "2021".into()]);
    let formatted = render_file(&file, &RustRules, &formatter);
    assert!(
        formatted.warning.is_none(),
        "rustfmt must format cleanly: {:?}",
        formatted.warning
    );

    std::fs::write(dir.join("src/models.rs"), &formatted.text).expect("write models.rs");

    // A compile error here is a generation bug; the driver asserts the wire cases.
    let run = Command::new("cargo")
        .args(["run", "--quiet"])
        .current_dir(&dir)
        .output()
        .expect("run cargo");
    assert!(
        run.status.success(),
        "generated crate failed to build or run:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("ROUNDTRIP_OK"),
        "driver did not report success:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

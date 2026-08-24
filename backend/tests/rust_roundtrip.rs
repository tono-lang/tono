//! End-to-end check that the Rust the engine emits compiles and round-trips.
//!
//! Generates a module, formats it with rustfmt, writes it as the `models` module
//! of a small out-of-workspace crate, and runs that crate's driver with cargo.
//! The driver asserts the hard wire cases hold (i64 above 2^53 as a string, bytes
//! as base64, internally-tagged union, open-enum lenient decode, canonical
//! round-trip). Skips cleanly if the toolchain is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::targets::rust::types::rust_casing;
use tono_backend::codegen::{generate_target, CodegenConfig, Formatter, TargetKind};
use tono_backend::ir::Model;

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

    // Generate the whole SDK and format it with the engine's formatter (rustfmt),
    // then write it as the harness crate's library. The drivers are separate
    // binaries, so they reach the SDK from outside it: a group the layout marks
    // internal is out of their reach, exactly as for any consumer.
    let model = Model {
        tono_ir_version: 6,
        modules: vec![demo_module()],
    };
    let files = generate_target(
        &model,
        TargetKind::Rust,
        &CodegenConfig::default(),
        &rust_casing(),
    )
    .expect("generates");
    let _ = std::fs::remove_dir_all(dir.join("src").join("models"));
    let formatter = Formatter::new("rustfmt", vec!["--edition".into(), "2021".into()]);
    for file in files {
        let formatted = formatter.run(&file.text);
        assert!(
            formatted.warning.is_none(),
            "rustfmt must format cleanly: {:?}",
            formatted.warning
        );
        // Generated paths already carry the crate's `src/` segment, so
        // stripping the target prefix lands them at the harness crate's root.
        let relative = file
            .path
            .strip_prefix(TargetKind::Rust.dir())
            .expect("target-rooted path");
        let out = dir.join(relative);
        std::fs::create_dir_all(out.parent().expect("a parent")).expect("create module dir");
        std::fs::write(&out, &formatted.text).expect("write models source");
    }

    // A compile error here is a generation bug; the driver asserts the wire cases.
    let run = Command::new("cargo")
        .args(["run", "--quiet", "--bin", "roundtrip"])
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

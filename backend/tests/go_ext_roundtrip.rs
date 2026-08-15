//! End-to-end check that the Go the engine emits for the `ext`/`extern` FFI
//! library block compiles against a real library: the module in
//! this test exercises a worked example (a config load with a per-field
//! `yields`/`returns` projection and a `match`, an injectable bus handle
//! with a construction fallback, and an op implemented by a call into the
//! handle's own method with a declared sentinel-to-error mapping).
//!
//! The verification model this test relies on is the target compiler, not a
//! Rust assertion: the generated Go reads `cfg.Host`/`ack.OK` directly off
//! whatever the stand-in libraries under `fixtures/` return, so a
//! `fixtures/` package that does not match the declared shape must fail
//! `go build`, not this harness. The negative test below proves that.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Both tests write into the same `codegen-tests/go-ext/` tree (the sdk
/// output, and — for the negative test — the fixture library itself), so
/// they cannot run concurrently.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

use tono_backend::codegen::modules::CodegenConfig;
use tono_backend::codegen::pipeline::generate_target;
use tono_backend::codegen::targets::go::entry::ext_fixtures::rfc0023_appendix_model;
use tono_backend::codegen::targets::go::types::go_casing;
use tono_backend::codegen::{Formatter, TargetKind};
use tono_backend::ir::Model;

fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/go-ext")
}

/// Generate the model for Go, gofmt every file, and write it under
/// `codegen-tests/go-ext/sdk/`, alongside a `go.mod` pointing the two `ext`
/// library import paths at the stand-in packages under `fixtures/` (via
/// `replace`, the same mechanism a real consumer's own `go.mod` would use
/// for a private/internal dependency).
fn write_sdk(model: &Model) -> PathBuf {
    let dir = fixtures_dir().join("sdk");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create sdk dir");

    let config = CodegenConfig {
        flatten: true,
        remap: vec![],
        go_module: None,
    };
    let casing = go_casing();
    let files = generate_target(model, TargetKind::Go, &config, &casing)
        .expect("generate_target(Go) must succeed for a well-formed ext model");
    assert!(!files.is_empty(), "expected generated Go files");

    for file in &files {
        let formatted = Formatter::new("gofmt", vec![]).run(&file.text);
        assert!(
            formatted.warning.is_none(),
            "gofmt must format {} cleanly: {:?}\n{}",
            file.path.display(),
            formatted.warning,
            file.text
        );
        let out = dir.join(&file.path);
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, formatted.text).unwrap();
    }

    std::fs::write(
        dir.join("go.mod"),
        "module tono-ext-fixture/sdk\n\n\
         go 1.21\n\n\
         require tono-ext-fixture/companyconfig v0.0.0\n\
         require tono-ext-fixture/companybus v0.0.0\n\n\
         replace tono-ext-fixture/companyconfig => ../fixtures/companyconfig\n\
         replace tono-ext-fixture/companybus => ../fixtures/companybus\n",
    )
    .unwrap();
    dir
}

#[test]
fn the_rfc_appendix_generates_go_that_compiles_against_the_real_libraries() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&rfc0023_appendix_model());
    let build = Command::new("go")
        .arg("build")
        .arg("./...")
        .current_dir(&dir)
        .output()
        .expect("run go build");
    assert!(
        build.status.success(),
        "generated Go failed to build:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// Declaring a field the library does not actually have (or reordering an
/// argument list with incompatible types) must break the Go build: the
/// declaration is a hypothesis the target compiler grades,
/// not a contract `tono` itself confirms. This renames the real
/// `companyconfig.Config.Host` field so the generated `cfg.Host` access no
/// longer compiles, without touching the generator at all.
#[test]
fn a_field_the_library_does_not_have_breaks_the_go_build() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&rfc0023_appendix_model());
    let config_go = fixtures_dir().join("fixtures/companyconfig/config.go");
    let original = std::fs::read_to_string(&config_go).unwrap();
    let broken = original.replace("Host", "Address");
    std::fs::write(&config_go, &broken).unwrap();
    let result = Command::new("go")
        .arg("build")
        .arg("./...")
        .current_dir(&dir)
        .output()
        .expect("run go build");
    std::fs::write(&config_go, &original).unwrap();
    assert!(
        !result.status.success(),
        "expected the build to fail once the library no longer has the declared field"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("Host"),
        "expected the compiler error to name the missing field:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// The appendix fixture's own two declared tests run and pass: the
/// generated hermetic test file swaps the foreign handle field's static type
/// for tono's own interface, fakes `companybus.publisher.send` through it,
/// and overrides the `companyconfig.load`/`companybus.connect` construction
/// calls outright, so neither declared test's assertions depend on the real
/// stand-in libraries answering correctly.
///
/// This does not yet prove the stronger claim the seam mechanism aims for
/// (a hermetic build succeeding with the real packages absent from
/// `go.mod` entirely): the always-built entry file still references
/// `companyconfig`/`companybus` directly on the production construction
/// path (`New`, not the seam), so removing the `require` lines still fails
/// `go build`. Closing that gap needs the build-tag file partition the plan
/// flags as the highest-risk remaining piece; see the task's own report for
/// what was tried.
#[test]
fn the_rfc_appendix_declared_tests_pass_hermetically() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&rfc0023_appendix_model());
    let test = Command::new("go")
        .arg("test")
        .arg("./...")
        .arg("-run")
        .arg("Test")
        .arg("-v")
        .current_dir(&dir)
        .output()
        .expect("run go test");
    assert!(
        test.status.success(),
        "the generated hermetic declared tests failed:\n{}\n{}",
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr)
    );
    let out = String::from_utf8_lossy(&test.stdout);
    assert!(
        out.contains("PASS"),
        "expected at least one passing test:\n{out}"
    );
}

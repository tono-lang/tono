//! End-to-end check that the Rust the engine emits for an `ext`/`extern`
//! FFI surface compiles against real crates: the model in this test
//! exercises a `companyconfig` library (a config load with a
//! per-field `yields`/`returns` projection and a `match`) and its
//! `companybus` opaque handle (a field typed by an `ext` block's own `type`,
//! constructed by a free call, and an op's own `impl .bus.send(..)` body
//! calling through it).
//!
//! The verification model this test relies on is the target compiler, not a
//! Rust assertion: the generated code reads `cfg.host`/`cfg.credentials.secret`
//! directly off whatever the stand-in crate under `fixtures/` returns, so a
//! `fixtures/` crate that does not match the declared shape must fail
//! `cargo build`, not this harness. The negative test below proves that.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

/// Both tests write into the same `codegen-tests/rust-ext/` tree (the sdk
/// output, and, for the negative test, the fixture crate itself), so they
/// cannot run concurrently.
static FIXTURE_LOCK: Mutex<()> = Mutex::new(());

use tono_backend::codegen::fixtures::handle_source::handle_source_model;
use tono_backend::codegen::fixtures::per_lang_handle::per_lang_handle_model;
use tono_backend::codegen::pipeline::generate_target;
use tono_backend::codegen::targets::rust::entry::ext_fixtures::rust_ext_fixture_model;
use tono_backend::codegen::targets::rust::types::rust_casing;
use tono_backend::codegen::{CodegenConfig, TargetKind};
use tono_backend::ir::Model;

fn have_cargo() -> bool {
    Command::new("cargo")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn root_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/rust-ext")
}

/// Generate the model for Rust and write it under `codegen-tests/rust-ext/
/// sdk/src/`, alongside a `Cargo.toml` pointing the `companyconfig` `ext`
/// library import at the stand-in crate under `fixtures/` via a path
/// dependency (the same mechanism a real consumer's own `Cargo.toml` would
/// use once the manifest can declare the dependency itself).
fn write_sdk(model: &Model) -> PathBuf {
    let dir = root_dir().join("sdk");
    let _ = std::fs::remove_dir_all(&dir);
    let src = dir.join("src");
    std::fs::create_dir_all(&src).expect("create sdk/src dir");

    let config = CodegenConfig::default();
    let casing = rust_casing();
    let files = generate_target(model, TargetKind::Rust, &config, &casing)
        .expect("generate_target(Rust) must succeed for a well-formed ext model");
    assert!(!files.is_empty(), "expected generated Rust files");

    for file in &files {
        // Every generated path is rooted at "rust/" and already carries the
        // crate's `src/` segment, so stripping the target prefix lays the
        // sources out at the sdk crate's own root.
        let rel = file
            .path
            .strip_prefix("rust")
            .unwrap_or(file.path.as_path());
        let out = dir.join(rel);
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        std::fs::write(out, &file.text).unwrap();
    }

    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\n\
         name = \"sdk\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         publish = false\n\n\
         [dependencies]\n\
         serde = { version = \"1\", features = [\"derive\"] }\n\
         serde_json = \"1\"\n\
         companyconfig = { path = \"../fixtures/companyconfig\" }\n\
         companybus = { path = \"../fixtures/companybus\" }\n\
         envkit = { path = \"../fixtures/envkit\" }\n\
         keepkit = { path = \"../fixtures/keepkit\" }\n\n\
         [features]\n\
         reqwest = []\n\n\
         [workspace]\n",
    )
    .unwrap();
    dir
}

#[test]
fn the_appendix_worked_example_generates_rust_that_compiles_against_the_real_crate() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test rust_ext_roundtrip`");
        return;
    }
    if !have_cargo() {
        eprintln!("skipping: cargo not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&rust_ext_fixture_model());
    let build = Command::new("cargo")
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("run cargo build");
    assert!(
        build.status.success(),
        "generated Rust failed to build:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// Declaring a field the library does not actually have must break the
/// Rust build: the declaration is a hypothesis the target compiler grades,
/// not a contract `tono` itself confirms. This renames the real
/// `companyconfig::Config::host` field so the generated `cfg.host` access
/// no longer compiles, without touching the generator at all.
#[test]
fn a_field_the_library_does_not_have_breaks_the_rust_build() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test rust_ext_roundtrip`");
        return;
    }
    if !have_cargo() {
        eprintln!("skipping: cargo not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&rust_ext_fixture_model());
    let lib_rs = root_dir().join("fixtures/companyconfig/src/lib.rs");
    let original = std::fs::read_to_string(&lib_rs).unwrap();
    let broken = original.replace("host", "address");
    std::fs::write(&lib_rs, &broken).unwrap();
    let result = Command::new("cargo")
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("run cargo build");
    std::fs::write(&lib_rs, &original).unwrap();
    assert!(
        !result.status.success(),
        "expected the build to fail once the crate no longer has the declared field"
    );
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("host"),
        "expected the compiler error to name the missing field:\n{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

/// A field sourced from a foreign handle's method (`config: cfg =
/// .provider.get()`) feeding several `@http` operations, an injectable
/// second read with an argument, and an op's own `impl` body reading the
/// same handle: the whole entry compiles against the real stand-in crate,
/// which is what proves the receiver borrow, the awaited call, and the
/// shared (non-`Clone`) handle agree beyond rendered text.
#[test]
fn a_field_sourced_from_a_handle_method_compiles_against_the_real_crate() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test rust_ext_roundtrip`");
        return;
    }
    if !have_cargo() {
        eprintln!("skipping: cargo not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&handle_source_model("rust"));
    let build = Command::new("cargo")
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("run cargo build");
    assert!(
        build.status.success(),
        "generated Rust failed to build:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

/// One logical handle whose instantiation names a different foreign type
/// per language: the "rust" entry spells the crate's own `Vault<String>`,
/// verbatim, while the Go sibling names a `Store[T]`. Compiling against the
/// real stand-in crate is what proves the per-language spelling resolves
/// (the Go half of the same claim lives in `go_ext_roundtrip.rs`).
#[test]
fn a_handle_naming_its_foreign_type_per_language_compiles_against_the_real_crate() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test rust_ext_roundtrip`");
        return;
    }
    if !have_cargo() {
        eprintln!("skipping: cargo not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&per_lang_handle_model("rust"));
    let build = Command::new("cargo")
        .arg("build")
        .current_dir(&dir)
        .output()
        .expect("run cargo build");
    assert!(
        build.status.success(),
        "generated Rust failed to build:\n{}\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr)
    );
}

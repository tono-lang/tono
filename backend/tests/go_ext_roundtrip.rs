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

use tono_backend::codegen::fixtures::handle_source::handle_source_model;
use tono_backend::codegen::modules::CodegenConfig;
use tono_backend::codegen::pipeline::generate_target;
use tono_backend::codegen::targets::go::entry::ext_fixtures::{
    composed_handles_model, ctx_extern_model, infallible_extern_model, reference_example_model,
};
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

/// The appendix fixture's `require`/`replace` lines: the two `ext` library
/// import paths pointed at the stand-in packages under `fixtures/`.
const APPENDIX_GO_MOD_DEPS: &str = "require tono-ext-fixture/companyconfig v0.0.0\n\
     require tono-ext-fixture/companybus v0.0.0\n\n\
     replace tono-ext-fixture/companyconfig => ../fixtures/companyconfig\n\
     replace tono-ext-fixture/companybus => ../fixtures/companybus\n";

/// Generate the model for Go, gofmt every file, and write it under
/// `codegen-tests/go-ext/sdk/`, alongside a `go.mod` whose `mod_deps`
/// (`require`/`replace` lines) point each `ext` library import path at its
/// stand-in package under `fixtures/` (via `replace`, the same mechanism a
/// real consumer's own `go.mod` would use for a private/internal
/// dependency).
fn write_sdk(model: &Model, mod_deps: &str) -> PathBuf {
    write_sdk_with(model, mod_deps, None)
}

/// [`write_sdk`] with the generated SDK's own Go module path: an entry with
/// `@http` operations imports the shared `support` package across packages,
/// which Go can only spell through the module path (`--go-module`).
fn write_sdk_with(model: &Model, mod_deps: &str, go_module: Option<&str>) -> PathBuf {
    let dir = fixtures_dir().join("sdk");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create sdk dir");

    let config = CodegenConfig {
        flatten: true,
        remap: vec![],
        go_module: go_module.map(str::to_string),
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
        format!("module tono-ext-fixture/sdk\n\ngo 1.21\n\n{mod_deps}"),
    )
    .unwrap();
    dir
}

#[test]
fn the_appendix_worked_example_generates_go_that_compiles_against_the_real_libraries() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&reference_example_model(), APPENDIX_GO_MOD_DEPS);
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
    let dir = write_sdk(&reference_example_model(), APPENDIX_GO_MOD_DEPS);
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
/// `go build`. Closing that gap needs a build-tag file partition, the
/// highest-risk remaining piece of that stronger claim, still open.
#[test]
fn the_appendix_worked_example_declared_tests_pass_hermetically() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&reference_example_model(), APPENDIX_GO_MOD_DEPS);
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

const COMPOSE_GO_MOD_DEPS: &str = "require tono-ext-fixture/compose v0.0.0\n\n\
     replace tono-ext-fixture/compose => ../fixtures/compose\n";

/// A field-typed handle passed as another extern call's own argument
/// (`new_combined(.b, .primary, .secondary)`), plus the same handle type
/// injected via `@arg` instead of constructed: two paths the appendix
/// fixture above never compiled. Before the fix this failed three
/// independent ways: `codec.go` referenced the library package without
/// importing it (the adapter's own `real` field), `client.go` imported it
/// without using it (the storage-typed positions never needed the import),
/// the generated constructor named an undefined type instead of the
/// storage interface, and the sibling `primary`/`secondary` arguments
/// passed tono's own adapter into `NewCombined` instead of the real
/// `*compose.Resource` value it declares.
#[test]
fn a_composed_and_an_injected_handle_both_build() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&composed_handles_model(), COMPOSE_GO_MOD_DEPS);
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

/// `go build` alone proves the *shape* compiles, not that it runs safely: an
/// `@arg`-injected handle holds whatever the caller supplied, which is not
/// guaranteed to be tono's own generated adapter (a hermetic test's fake,
/// most obviously). Before the unwrap in `sibling_path_expr` was gated to
/// constructed fields only, `New` compiled fine but a non-adapter `injected`
/// value panicked the first time any *sibling* field's construction call
/// tried to unwrap it — this only happens to a *different* field than the
/// one holding the fake, so a build-only proof can never catch it. This test
/// hand-authors exactly such a fake, passes it as `injected`, and drives
/// both the injected and the fully-composed op through a real `go test` run.
#[test]
fn an_injected_fake_handle_runs_without_panicking() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(&composed_handles_model(), COMPOSE_GO_MOD_DEPS);
    let test_go = dir.join("go/t/injected_fake_test.go");
    std::fs::write(
        &test_go,
        r#"package t

import (
	"context"
	"testing"
)

// fakeResource is authored by an SDK consumer, never by tono: it satisfies
// the generated interface structurally, without embedding tono's own
// unexported adapter type, exactly the shape an unconditional type
// assertion in a sibling field's construction call used to panic on.
type fakeResource struct{ data string }

func (f fakeResource) Get() (Value, error) { return Value{Data: f.data}, nil }

func TestInjectedFakeHandleRunsWithoutTheRealLibrary(t *testing.T) {
	c, err := New("a", "b", "c", "d", fakeResource{data: "fake-value"})
	if err != nil {
		t.Fatalf("New: %v", err)
	}

	injected, err := c.InjectedValue(context.Background())
	if err != nil {
		t.Fatalf("InjectedValue: %v", err)
	}
	if injected.Data != "fake-value" {
		t.Fatalf("InjectedValue: got %q, want %q", injected.Data, "fake-value")
	}

	// The composed op reads sibling fields (`primary`/`secondary`) that
	// were actually constructed against the real library, not the fake
	// standing in for `injected`: this is what used to panic, since the
	// old unconditional unwrap did not distinguish the two.
	combined, err := c.CombinedValue(context.Background())
	if err != nil {
		t.Fatalf("CombinedValue: %v", err)
	}
	if combined.Data == "" {
		t.Fatalf("CombinedValue: expected a value from the real composed chain")
	}
}
"#,
    )
    .unwrap();

    let test = Command::new("go")
        .arg("test")
        .arg("./...")
        .arg("-run")
        .arg("TestInjectedFakeHandleRunsWithoutTheRealLibrary")
        .arg("-v")
        .current_dir(&dir)
        .output()
        .expect("run go test");
    std::fs::remove_file(&test_go).unwrap();
    assert!(
        test.status.success(),
        "the injected fake handle must run without panicking:\n{}\n{}",
        String::from_utf8_lossy(&test.stdout),
        String::from_utf8_lossy(&test.stderr)
    );
    let out = String::from_utf8_lossy(&test.stdout);
    assert!(
        !out.contains("panic:"),
        "expected no panic, got:\n{out}\n{}",
        String::from_utf8_lossy(&test.stderr)
    );
}

/// A single-return foreign function with no error, matching a real API like
/// `uuid.NewString() string`, declared `infallible` and bound with no
/// `yields:` at all. Before the fix this always destructured two Go values
/// regardless of the real function's own arity, so the generated SDK failed
/// to build with "assignment mismatch: 2 variables but idgen.NewString
/// returns 1 value".
#[test]
fn an_infallible_single_return_extern_builds() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(
        &infallible_extern_model(),
        "require tono-ext-fixture/idgen v0.0.0\n\n\
         replace tono-ext-fixture/idgen => ../fixtures/idgen\n",
    );
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

/// A foreign handle's `ctx`-marked method call builds against a real
/// library whose method takes `context.Context` as its first parameter
/// (Go's own idiom): proves the generated interface, adapter, and op-body
/// call site all agree on the added `ctx` parameter well enough to compile,
/// not just to render matching text.
#[test]
fn a_ctx_marked_handle_method_builds() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk(
        &ctx_extern_model(),
        "require tono-ext-fixture/svc v0.0.0\n\n\
         replace tono-ext-fixture/svc => ../fixtures/svc\n",
    );
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

/// A field sourced from a foreign handle's method (`config: cfg =
/// .provider.get()`) feeding several `@http` operations, an injectable
/// second read with an argument, and an op's own `impl` body reading the
/// same handle: the whole entry compiles against the real stand-in
/// library, which is what proves the interface call, the `ctx` slot filled
/// at construction, and the shared receiver agree beyond rendered text.
#[test]
fn a_field_sourced_from_a_handle_method_builds() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_ext_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let _guard = FIXTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = write_sdk_with(
        &handle_source_model("go"),
        "require tono-ext-fixture/envkit v0.0.0\n\n\
         replace tono-ext-fixture/envkit => ../fixtures/envkit\n",
        Some("tono-ext-fixture/sdk/go"),
    );
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

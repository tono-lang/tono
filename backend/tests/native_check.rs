//! End-to-end proof that the scaffolding produces projects the real toolchains
//! accept: generate a small SDK, run the native checker, and assert the verdict.
//!
//! These build with `cargo`, `go`, and `tsc`, so they are `#[ignore]`d by default
//! and run on demand (`cargo test -p tono-backend --test native_check -- --ignored`)
//! in an environment that has the toolchains. The unit tests in `codegen::check`
//! cover the scaffolding and verdict mapping without them.

use std::path::PathBuf;

use tono_backend::codegen::{check, generate, CheckOutcome, CodegenConfig, TargetKind};
use tono_backend::ir::decode_model;

/// A one-module model with a single `i64` field. The wide integer forces the
/// serde split (a Rust helper module, TypeScript codecs), so the check exercises
/// a multi-file layout, not just a lone type.
const IR: &str = r#"{"tono_ir_version":9,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-native-check-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn generated(target: TargetKind) -> Vec<tono_backend::codegen::GeneratedFile> {
    let model = decode_model(IR).expect("decode IR");
    generate(&model, &[target], &CodegenConfig::default()).expect("generate")
}

#[test]
#[ignore = "requires the rust toolchain (cargo)"]
fn rust_sdk_compiles() {
    let dir = scratch("rust");
    let outcome = check(TargetKind::Rust, &generated(TargetKind::Rust), &dir).expect("scaffold");
    assert_eq!(
        outcome,
        CheckOutcome::Compiles,
        "generated Rust must compile"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires the go toolchain"]
fn go_sdk_compiles() {
    let dir = scratch("go");
    let outcome = check(TargetKind::Go, &generated(TargetKind::Go), &dir).expect("scaffold");
    assert_eq!(outcome, CheckOutcome::Compiles, "generated Go must compile");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "requires the typescript toolchain (tsc)"]
fn typescript_sdk_compiles() {
    let dir = scratch("ts");
    let outcome = check(
        TargetKind::TypeScript,
        &generated(TargetKind::TypeScript),
        &dir,
    )
    .expect("scaffold");
    // Accept a missing tsc (it is often only a project-local dependency): the
    // point is that when it runs, it accepts the source.
    assert!(
        matches!(
            outcome,
            CheckOutcome::Compiles | CheckOutcome::ToolchainMissing { .. }
        ),
        "generated TypeScript must compile when tsc is available, got {outcome:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

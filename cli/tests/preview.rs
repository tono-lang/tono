//! End-to-end checks of `tono preview --once`: the non-interactive pass that reads
//! a `.tono`, renders the SDK, and reports a verdict. The interactive TUI is not
//! exercised here (it needs a terminal); these cover argument handling and the
//! render pipeline through the process boundary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-preview-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::write(path, contents).unwrap();
}

/// A fake `tono-frontend` that ignores its arguments and prints canned IR JSON, so
/// the render pipeline can be exercised without the real OCaml binary or any target
/// toolchain. The generated code is produced before the compile-check, so this
/// reaches the generated pane regardless of what is installed.
#[cfg(unix)]
fn fake_frontend(dir: &Path, ir: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let script = dir.join("fake-frontend.sh");
    write(
        &script,
        &format!("#!/bin/sh\ncat <<'IR_EOF'\n{ir}\nIR_EOF\n"),
    );
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    script
}

const IR: &str = r#"{"tono_ir_version":18,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

#[test]
fn preview_requires_a_target() {
    let dir = tmpdir("needs-target");
    let source = dir.join("demo.tono");
    write(&source, "struct demo {}\n");
    let out = tono()
        .args(["preview", source.to_str().unwrap(), "--once"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing --target"));
}

#[test]
fn preview_rejects_an_unknown_target() {
    let dir = tmpdir("bad-target");
    let source = dir.join("demo.tono");
    write(&source, "struct demo {}\n");
    let out = tono()
        .args([
            "preview",
            source.to_str().unwrap(),
            "--target",
            "java",
            "--once",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown target: java"));
}

#[test]
fn preview_reports_a_missing_frontend() {
    // The source exists, so the pipeline gets past the read and stops at the
    // frontend: a bogus binary fails the pass with a message naming the fix,
    // not a bare crash.
    let dir = tmpdir("no-frontend");
    let source = dir.join("demo.tono");
    write(&source, "struct demo {}\n");
    let out = tono()
        .args([
            "preview",
            source.to_str().unwrap(),
            "--target",
            "rust",
            "--once",
        ])
        .env("TONO_FRONTEND", "tono-no-such-frontend-xyz")
        .output()
        .unwrap();
    assert!(!out.status.success(), "a missing frontend fails the pass");
    assert!(String::from_utf8_lossy(&out.stderr).contains("not found"));
}

#[test]
#[cfg(unix)]
fn preview_renders_the_generated_sdk() {
    // With a fake frontend supplying IR, the pass must render the target's source
    // whatever toolchain is (or is not) installed. TypeScript is used so the check
    // stays cheap: `tsc` is typically only a project-local dependency, so the pass
    // reaches a verdict without a heavyweight native build.
    let dir = tmpdir("renders");
    let source = dir.join("demo.tono");
    write(&source, "struct charge { amount: i64 }\n");
    let frontend = fake_frontend(&dir, IR);
    let out = tono()
        .args([
            "preview",
            source.to_str().unwrap(),
            "--target",
            "ts",
            "--once",
        ])
        .env("TONO_FRONTEND", &frontend)
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("== target: typescript =="));
    assert!(stdout.contains("export interface Charge"));
    assert!(
        stdout.contains("compiles: yes") || stdout.contains("tsc not found (skipped)"),
        "reports a check outcome:\n{stdout}"
    );
}

// Every IR fixture literal in this file embeds a bare version number; a
// stale one fails with a decode error far from this assertion. Catch it
// here instead: the same bump every past IR version change forgot.
#[test]
fn ir_fixture_uses_the_current_ir_version() {
    let expected = format!("\"tono_ir_version\":{}", tono_backend::ir::TONO_IR_VERSION);
    assert!(IR.contains(&expected), "stale tono_ir_version in IR: {IR}");
}

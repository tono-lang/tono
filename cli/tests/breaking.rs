//! End-to-end checks of `tono breaking`: the compat gate against a git baseline.
//!
//! Each test builds a throwaway git repo, commits a baseline IR, then runs the
//! command against a modified working copy. The baseline is read straight from the
//! commit (`git show <ref>:<path>`), exactly as in a real check.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The baseline IR: one structure with an `i64` amount.
const BASELINE: &str = r#"{"tono_ir_version":2,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// The current IR: `amount` retyped to a string, a wire break.
const RETYPED: &str = r#"{"tono_ir_version":2,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"string"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Build a fresh repo whose `HEAD` commits `BASELINE` as `ir.json`.
fn repo(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-breaking-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("ir.json"), BASELINE).unwrap();
    git(&dir, &["add", "ir.json"]);
    git(&dir, &["commit", "-q", "-m", "baseline"]);
    dir
}

/// Run `tono breaking <current> --baseline HEAD --baseline-path ir.json` (plus any
/// extra flags) in the repo, returning (success, stdout).
fn breaking(dir: &Path, extra: &[&str]) -> (bool, String) {
    std::fs::write(dir.join("current.json"), RETYPED).unwrap();
    let mut args = vec![
        "breaking",
        "current.json",
        "--baseline",
        "HEAD",
        "--baseline-path",
        "ir.json",
    ];
    args.extend_from_slice(extra);
    let out = tono().current_dir(dir).args(&args).output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_wire_break_is_reported_and_fails_the_gate() {
    let dir = repo("break");
    let (ok, stdout) = breaking(&dir, &[]);
    assert!(!ok, "a wire break must fail the gate");
    assert!(
        stdout.contains("error: retype demo#Charge.amount"),
        "the retype is classified and printed: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_allowlisted_break_passes() {
    let dir = repo("allow");
    let (ok, stdout) = breaking(&dir, &["--allow", "retype demo#Charge.amount"]);
    assert!(ok, "the allowlisted break passes: {stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_level_override_downgrades_the_break_to_a_warning() {
    let dir = repo("warn");
    let (ok, stdout) = breaking(&dir, &["--level", "wire-breaking=warn"]);
    assert!(ok, "a warning does not fail the gate: {stdout}");
    assert!(
        stdout.contains("warn: retype demo#Charge.amount"),
        "{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_identical_current_ir_has_no_changes() {
    let dir = repo("same");
    // Overwrite the working copy with the baseline: nothing changed.
    std::fs::write(dir.join("current.json"), BASELINE).unwrap();
    let out = tono()
        .current_dir(&dir)
        .args([
            "breaking",
            "current.json",
            "--baseline",
            "HEAD",
            "--baseline-path",
            "ir.json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 error"), "{stdout}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_missing_baseline_ref_fails_cleanly() {
    let dir = repo("noref");
    std::fs::write(dir.join("current.json"), RETYPED).unwrap();
    let out = tono()
        .current_dir(&dir)
        .args([
            "breaking",
            "current.json",
            "--baseline",
            "v9.9.9-does-not-exist",
            "--baseline-path",
            "ir.json",
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an unknown ref fails rather than panics"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_mistyped_flag_is_rejected_not_swallowed_as_the_ir_path() {
    let dir = repo("badflag");
    std::fs::write(dir.join("current.json"), RETYPED).unwrap();
    let out = tono()
        .current_dir(&dir)
        .args([
            "breaking",
            "current.json",
            "--baselin", // typo: must error, not become a second positional
            "HEAD",
            "--baseline-path",
            "ir.json",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a mistyped flag must fail");
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown flag"));
    let _ = std::fs::remove_dir_all(&dir);
}

//! End-to-end checks of the `tono` binary: IR JSON in, SDK files out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const IR: &str = r#"{"tono_ir_version":2,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Run `tono gen` feeding `IR` on stdin; returns whether it succeeded.
fn gen_via_stdin(out: &Path, target: &str) -> bool {
    let mut child = tono()
        .args(["gen", "--target", target, "--out", out.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(IR.as_bytes())
        .unwrap();
    child.wait().unwrap().success()
}

#[test]
fn version_prints_the_version() {
    let out = tono().arg("version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("tono "));
}

#[test]
fn gen_writes_a_file_per_target_from_stdin() {
    let out = tmpdir("stdin");
    assert!(gen_via_stdin(&out, "rust,go,typescript"));
    for (sub, ext) in [("rust", "rs"), ("go", "go"), ("typescript", "ts")] {
        let path = out.join(sub).join(format!("demo.{ext}"));
        let text = std::fs::read_to_string(&path).expect("generated file exists");
        assert!(text.contains("DO NOT EDIT"), "{sub} carries the banner");
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_reads_the_ir_path_argument() {
    let out = tmpdir("path");
    std::fs::create_dir_all(&out).unwrap();
    let ir_path = out.join("ir.json");
    std::fs::write(&ir_path, IR).unwrap();
    let status = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            ir_path.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(out.join("rust").join("demo.rs").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_requires_a_target() {
    let status = tono()
        .args(["gen", "--out", "/tmp/unused"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_rejects_an_unknown_target() {
    let status = tono()
        .args(["gen", "--target", "java", "--out", "/tmp/unused"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_rejects_invalid_ir() {
    let out = tmpdir("bad");
    let mut child = tono()
        .args(["gen", "--target", "rust", "--out", out.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    assert!(!child.wait().unwrap().success());
}

#[test]
fn gen_flag_without_a_value_fails() {
    let status = tono()
        .args(["gen", "--target"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_missing_ir_file_fails() {
    let out = tmpdir("missing");
    let status = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            "/no/such/ir.json",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn unknown_command_fails() {
    let status = tono().arg("wat").stdin(Stdio::null()).status().unwrap();
    assert!(!status.success());
}

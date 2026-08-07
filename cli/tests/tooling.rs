//! End-to-end checks of `tono`'s source-level subcommands: `check` and `fmt`
//! delegate to the OCaml frontend, and `preview` renders the SDK and runs the
//! target toolchain. These need the built frontend binary; they skip cleanly
//! when it is absent so a backend-only checkout still passes.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The dune-built frontend binary. `TONO_FRONTEND` overrides it; otherwise we
/// look in the default build tree next to the workspace.
fn frontend() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TONO_FRONTEND") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let p = root.join("_build/default/frontend/bin/tono_frontend.exe");
    p.exists().then_some(p)
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

/// A `tono` invocation with the frontend wired in. Returns `None` (skip) when
/// the frontend binary is not built.
fn tono() -> Option<Command> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tono"));
    cmd.env("TONO_FRONTEND", frontend()?);
    Some(cmd)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-tooling-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_tono(dir: &Path, name: &str, src: &str) -> PathBuf {
    let p = dir.join(name);
    std::fs::write(&p, src).unwrap();
    p
}

macro_rules! skip_without_frontend {
    () => {
        match tono() {
            Some(c) => c,
            None => {
                eprintln!("skipping: frontend binary not built (set TONO_FRONTEND)");
                return;
            }
        }
    };
}

/// Run `tono preview <file> --target <targets> --out <dir>/scaffold` and return
/// the exit success plus captured stdout.
fn run_preview(mut cmd: Command, file: &Path, targets: &str, dir: &Path) -> (bool, String) {
    let out = cmd
        .args(["preview"])
        .arg(file)
        .args(["--target", targets, "--out"])
        .arg(dir.join("scaffold"))
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn check_reports_type_errors() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("check-bad");
    let file = write_tono(&dir, "bad.tono", "struct box { it: missing }\n");
    let out = cmd.args(["check"]).arg(&file).output().unwrap();
    assert!(
        !out.status.success(),
        "an unresolved type must fail the check"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn check_clean_source_succeeds() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("check-ok");
    let file = write_tono(&dir, "ok.tono", "struct point { x: i64\n y: i64 }\n");
    let out = cmd.args(["check"]).arg(&file).output().unwrap();
    assert!(out.status.success(), "a clean file checks green");
    assert!(out.stdout.is_empty(), "check emits no IR");
    let _ = std::fs::remove_dir_all(&dir);
}

/// The whole point of the source path: a project with a manifest generates its
/// SDKs from its own `.tono` files, with no separate compile step and no IR
/// argument. Every module under `project.root` lands in the output.
#[test]
fn gen_compiles_the_project_sources_when_no_ir_is_given() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("gen-from-source");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write_tono(
        &dir.join("src"),
        "payments.tono",
        "struct charge { amount: i64 }\n",
    );
    write_tono(
        &dir.join("src"),
        "billing.tono",
        "struct invoice { total: i64 }\n",
    );

    let init = Command::new(env!("CARGO_BIN_EXE_tono"))
        .current_dir(&dir)
        .args(["init", "--yes", "--target", "rust"])
        .output()
        .unwrap();
    assert!(init.status.success());

    // Stdin closed, as in a script: nothing is piped, so the sources are used.
    let out = cmd
        .current_dir(&dir)
        .arg("gen")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "gen from sources failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    for module in ["payments", "billing"] {
        assert!(
            dir.join("dist/rust")
                .join(module)
                .join("types.rs")
                .is_file(),
            "module {module} was not generated"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Piped IR still wins over the sources on disk, so an existing pipeline that
/// feeds `tono gen` keeps generating exactly what it fed in.
#[test]
fn gen_prefers_piped_ir_over_the_project_sources() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("gen-pipe-wins");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    write_tono(
        &dir.join("src"),
        "ondisk.tono",
        "struct charge { amount: i64 }\n",
    );

    let init = Command::new(env!("CARGO_BIN_EXE_tono"))
        .current_dir(&dir)
        .args(["init", "--yes", "--target", "rust"])
        .output()
        .unwrap();
    assert!(init.status.success());

    const PIPED: &str =
        r#"{"tono_ir_version":10,"modules":[{"name":"piped","shapes":[],"operations":[]}]}"#;
    let mut child = cmd
        .current_dir(&dir)
        .arg("gen")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write as _;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(PIPED.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());

    assert!(
        !dir.join("rust/ondisk").exists(),
        "the source module must not be generated"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A root with nothing to compile is reported, not silently generated as an
/// empty SDK.
#[test]
fn gen_reports_a_source_root_with_no_files() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("gen-no-sources");
    let init = Command::new(env!("CARGO_BIN_EXE_tono"))
        .current_dir(&dir)
        .args(["init", "--yes", "--target", "rust"])
        .output()
        .unwrap();
    assert!(init.status.success());

    let out = cmd
        .current_dir(&dir)
        .arg("gen")
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no .tono files"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fmt_prints_canonical_source() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("fmt");
    let file = write_tono(&dir, "messy.tono", "struct point { x: i64, y: i64 }\n");
    let out = cmd.args(["fmt"]).arg(&file).output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        text.contains("struct point {\n  x: i64\n  y: i64\n}"),
        "got:\n{text}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_renders_and_compiles_each_target() {
    // One case per compilable target: the rendered marker proves the right
    // language was emitted, the checker line proves its toolchain ran. cargo
    // is always present (this test runs under it); go is probed.
    let cases = [
        ("go", "type Charge struct", "compiles: yes (go build)"),
        ("rust", "pub struct Charge", "compiles: yes (cargo check)"),
    ];
    for (target, rendered, checked) in cases {
        let cmd = skip_without_frontend!();
        if target == "go" && !have("go") {
            eprintln!("skipping {target}: toolchain not available");
            continue;
        }
        let dir = tmp(&format!("preview-{target}"));
        let file = write_tono(
            &dir,
            "charge.tono",
            "struct charge { amount: i64\n note: string? }\n",
        );
        let (ok, stdout) = run_preview(cmd, &file, target, &dir);
        assert!(ok, "{target} preview failed:\n{stdout}");
        assert!(stdout.contains(rendered), "renders {target}:\n{stdout}");
        assert!(stdout.contains(checked), "checks {target}:\n{stdout}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn preview_renders_typescript_even_without_tsc() {
    let cmd = skip_without_frontend!();
    let dir = tmp("preview-ts");
    let file = write_tono(&dir, "charge.tono", "struct charge { amount: i64 }\n");
    // Rendering always happens; the check either compiles or reports a skip, but
    // an absent toolchain never fails the command.
    let (ok, stdout) = run_preview(cmd, &file, "typescript", &dir);
    assert!(
        ok,
        "preview must not fail on a missing toolchain:\n{stdout}"
    );
    assert!(
        stdout.contains("interface Charge"),
        "renders the SDK:\n{stdout}"
    );
    assert!(
        stdout.contains("compiles: yes") || stdout.contains("tsc not found (skipped)"),
        "reports a check outcome:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn frontend_is_discovered_without_env() {
    // The dev-build discovery path: no TONO_FRONTEND, the binary is found in
    // the dune build tree next to the workspace.
    if frontend().is_none() {
        eprintln!("skipping: frontend binary not built");
        return;
    }
    let dir = tmp("discover");
    let file = write_tono(&dir, "ok.tono", "struct point { x: i64 }\n");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tono"));
    cmd.env_remove("TONO_FRONTEND");
    let out = cmd.args(["check"]).arg(&file).output().unwrap();
    assert!(
        out.status.success(),
        "check finds the frontend without TONO_FRONTEND:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_skips_missing_typescript_toolchain() {
    // A minimal PATH guarantees `tsc` is absent, so the skip path is
    // deterministic regardless of what the host has installed.
    let mut cmd = skip_without_frontend!();
    let dir = tmp("preview-no-tsc");
    let file = write_tono(&dir, "charge.tono", "struct charge { amount: i64 }\n");
    cmd.env("PATH", "/usr/bin:/bin");
    let (ok, stdout) = run_preview(cmd, &file, "typescript", &dir);
    assert!(ok, "a missing toolchain is not a failure");
    assert!(
        stdout.contains("compile-check: tsc not found (skipped)"),
        "reports the skip:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_watch_reruns_and_stops_when_file_is_removed() {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    let mut cmd = skip_without_frontend!();
    if !have("go") {
        eprintln!("skipping: go toolchain not available");
        return;
    }
    let dir = tmp("preview-watch");
    let file = write_tono(&dir, "w.tono", "struct a { x: i64 }\n");
    let mut child = cmd
        .args(["preview"])
        .arg(&file)
        .args(["--target", "go", "--out"])
        .arg(dir.join("scaffold"))
        .arg("--watch")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    // The watch is event-driven, so the test is too: each step waits for the
    // child to report the previous render instead of guessing with sleeps
    // (a slow runner would otherwise race the second build).
    let (tx, rx) = mpsc::channel::<String>();
    let pipe = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut all = String::new();
        for line in BufReader::new(pipe).lines() {
            let Ok(line) = line else { break };
            all.push_str(&line);
            all.push('\n');
            let _ = tx.send(line);
        }
        all
    });
    let wait_for = |needle: &str| {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(line) if line.contains(needle) => return,
                Ok(_) => {}
                Err(_) => panic!("timed out waiting for {needle:?}"),
            }
        }
    };
    wait_for("compiles:");
    std::fs::write(&file, "struct a { x: i64\n y: string }\n").unwrap();
    wait_for("compiles:");
    std::fs::remove_file(&file).unwrap();
    let mut waited = 0;
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(200));
        waited += 200;
        if waited > 30_000 {
            let _ = child.kill();
            panic!("watch did not stop after the file was removed");
        }
    };
    let stdout = reader.join().unwrap();
    assert!(status.success(), "watch exits cleanly:\n{stdout}");
    assert_eq!(
        stdout.matches("--- preview").count(),
        2,
        "one render per save:\n{stdout}"
    );
    assert!(
        stdout.contains("watch stopped"),
        "reports the stop:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_handles_multiple_targets() {
    let cmd = skip_without_frontend!();
    if !have("go") {
        eprintln!("skipping: go toolchain not available");
        return;
    }
    let dir = tmp("preview-multi");
    let file = write_tono(&dir, "charge.tono", "struct charge { amount: i64 }\n");
    let (ok, stdout) = run_preview(cmd, &file, "go,typescript", &dir);
    assert!(ok, "multi-target preview:\n{stdout}");
    assert!(stdout.contains("== target: go =="), "renders go:\n{stdout}");
    assert!(
        stdout.contains("== target: typescript =="),
        "renders ts:\n{stdout}"
    );
    assert!(
        stdout.contains("compiles: yes (go build)"),
        "checks go:\n{stdout}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_surfaces_frontend_errors() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("preview-bad");
    let file = write_tono(&dir, "bad.tono", "struct box { it: missing }\n");
    let out = cmd
        .args(["preview"])
        .arg(&file)
        .args(["--target", "go"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "a source error must fail preview");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn preview_requires_a_target() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("preview-notarget");
    let file = write_tono(&dir, "ok.tono", "struct point { x: i64 }\n");
    let out = cmd.args(["preview"]).arg(&file).output().unwrap();
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&dir);
}

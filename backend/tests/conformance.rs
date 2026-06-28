//! Cross-language wire conformance: the same IR through the TypeScript, Rust, and
//! Go targets must produce JSON that is canonically equal (parse to `Value`, then
//! compare — key order is insignificant) for a shared fixture.
//!
//! Each language is generated from one shared module, then a conformance driver
//! decodes a canonical wire document from stdin and re-encodes it. The re-encoded
//! JSON of every available language must equal the canonical input (and thus each
//! other). A language whose toolchain is absent is skipped; the test still
//! asserts conformance across whatever is present. Skipped under coverage.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tono_backend::codegen::render::render_file;
use tono_backend::codegen::targets::{go, rust, typescript};
use tono_backend::codegen::Formatter;

mod common;
use common::{matrix_module as shared_module, CANONICAL_WIRE as CANONICAL};

fn tests_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests")
}

fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Run a command in `dir`, optionally piping `input` to its stdin, and return its
/// stdout on success.
fn run(dir: &Path, program: &str, args: &[&str], input: Option<&str>) -> String {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn driver");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input.as_bytes())
            .expect("write stdin");
    }
    let out = child.wait_with_output().expect("wait driver");
    assert!(
        out.status.success(),
        "{program} {args:?} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8 stdout")
}

fn rust_output() -> Option<Value> {
    if !have("cargo", "--version") {
        return None;
    }
    let dir = tests_dir().join("rust");
    let file = rust::emit::emit_module(&shared_module(), &rust::types::rust_casing());
    let text = render_file(&file, &rust::RustRules, &Formatter::new("cat", vec![])).text;
    std::fs::write(dir.join("src/models.rs"), text).expect("write models.rs");
    let out = run(
        &dir,
        "cargo",
        &["run", "--quiet", "--bin", "conformance"],
        Some(CANONICAL),
    );
    Some(serde_json::from_str(out.trim()).expect("rust output is json"))
}

fn go_output() -> Option<Value> {
    if !have("go", "version") {
        return None;
    }
    let dir = tests_dir().join("go");
    let file = go::emit::emit_module(&shared_module(), &go::types::go_casing());
    let rough = render_file(&file, &go::GoRules, &Formatter::new("cat", vec![])).text;
    let source = format!("{}\n{}", go::emit::package_clause("main"), rough);
    let formatted = Formatter::new("gofmt", vec![]).run(&source);
    std::fs::write(dir.join("models.go"), formatted.text).expect("write models.go");
    let out = run(
        &dir,
        "go",
        &["run", "-tags", "conformance", "."],
        Some(CANONICAL),
    );
    Some(serde_json::from_str(out.trim()).expect("go output is json"))
}

/// The TypeScript conformance driver. The canonical input is embedded (so the
/// driver needs no Node type declarations to read stdin); it decodes then
/// re-encodes and prints the wire JSON.
fn ts_driver() -> String {
    format!(
        "import {{ decodeAccount, encodeAccount }} from \"./models\";\n\
         const input: any = {CANONICAL};\n\
         console.log(JSON.stringify(encodeAccount(decodeAccount(input))));\n"
    )
}

fn ts_output() -> Option<Value> {
    let ws = tests_dir().join("typescript");
    let tsc = ws.join("node_modules/.bin/tsc");
    if !tsc.exists() || !have("node", "--version") {
        return None;
    }
    let work = ws.join("work-conformance");
    std::fs::create_dir_all(&work).expect("create work-conformance");
    let file = typescript::emit::emit_module(&shared_module(), &typescript::types::ts_casing());
    let text = render_file(&file, &typescript::TsRules, &Formatter::new("cat", vec![])).text;
    std::fs::write(work.join("models.ts"), text).expect("write models.ts");
    std::fs::write(work.join("conformance.ts"), ts_driver()).expect("write conformance.ts");
    let compile = Command::new(&tsc)
        .args([
            "work-conformance/models.ts",
            "work-conformance/conformance.ts",
            "--outDir",
            "work-conformance/dist",
            "--target",
            "ES2020",
            "--module",
            "commonjs",
            "--lib",
            "ES2020,DOM",
        ])
        .current_dir(&ws)
        .output()
        .expect("run tsc");
    assert!(
        compile.status.success(),
        "tsc failed:\n{}\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );
    let out = run(&ws, "node", &["work-conformance/dist/conformance.js"], None);
    Some(serde_json::from_str(out.trim()).expect("ts output is json"))
}

#[test]
fn the_three_targets_agree_on_the_wire() {
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test conformance`");
        return;
    }
    let canonical: Value = serde_json::from_str(CANONICAL).expect("canonical fixture is json");

    let outputs = [
        ("rust", rust_output()),
        ("go", go_output()),
        ("typescript", ts_output()),
    ];
    let present: Vec<&str> = outputs
        .iter()
        .filter_map(|(name, v)| v.as_ref().map(|_| *name))
        .collect();
    assert!(
        !present.is_empty(),
        "no language toolchain available to check conformance"
    );
    eprintln!("conformance checked across: {present:?}");

    for (name, output) in &outputs {
        if let Some(value) = output {
            assert_eq!(
                value, &canonical,
                "{name} re-encoded JSON is not canonically equal to the fixture"
            );
        }
    }
}

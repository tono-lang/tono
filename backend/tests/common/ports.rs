//! Batch runners for the generated ports. Each port is emitted from the shared
//! matrix module, then a per-language driver decodes and re-encodes a batch of
//! wire documents: one JSON document per stdin line, one re-encoded document per
//! stdout line. The golden and differential harnesses share this machinery so
//! both exercise the exact same generated code paths.
//!
//! A port whose toolchain is absent yields `None`; each harness decides how to
//! degrade (skip the port, but never silently pass with nothing checked).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::Value;
use tono_backend::codegen::render::render_file_with_companion;
use tono_backend::codegen::targets::{go, rust, typescript};
use tono_backend::codegen::Formatter;

use super::matrix_module as shared_module;

/// The designated bespoke reference (RFC-0009): the golden vectors are generated
/// from this port, and every other port is checked against them.
pub const REFERENCE_PORT: &str = "rust";

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

/// One JSON document per input, newline-separated, for the stdin-driven drivers.
fn batch_stdin(inputs: &[Value]) -> String {
    let mut text = String::new();
    for input in inputs {
        text.push_str(&serde_json::to_string(input).expect("encode input"));
        text.push('\n');
    }
    text
}

/// Parse a driver's stdout: one re-encoded JSON document per line, in input
/// order. The count must match, so a driver cannot silently drop a document.
fn parse_batch(port: &str, inputs: &[Value], stdout: &str) -> Vec<Value> {
    let outputs: Vec<Value> = stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{port} output line is not json ({e}): {line}"))
        })
        .collect();
    assert_eq!(
        outputs.len(),
        inputs.len(),
        "{port} driver returned {} documents for {} inputs",
        outputs.len(),
        inputs.len()
    );
    outputs
}

fn rust_outputs(inputs: &[Value]) -> Option<Vec<Value>> {
    if !have("cargo", "--version") {
        return None;
    }
    let dir = tests_dir().join("rust");
    // Rust splits the module into types and serde files; write both as the
    // `models`/`models_serde` modules the harness crate declares.
    for module_file in rust::emit::emit_module(&shared_module(), &rust::types::rust_casing()) {
        let text = render_file_with_companion(
            &module_file.file,
            module_file.imports_companion.as_deref(),
            &rust::RustRules,
            &Formatter::new("cat", vec![]),
        )
        .text;
        std::fs::write(
            dir.join(format!("src/models{}.rs", module_file.suffix)),
            text,
        )
        .expect("write rust models source");
    }
    let out = run(
        &dir,
        "cargo",
        &["run", "--quiet", "--bin", "conformance"],
        Some(&batch_stdin(inputs)),
    );
    Some(parse_batch("rust", inputs, &out))
}

fn go_outputs(inputs: &[Value]) -> Option<Vec<Value>> {
    if !have("go", "version") {
        return None;
    }
    let dir = tests_dir().join("go");
    // Go emits separate types and serde files; both compile in one `package main`,
    // so write each into the harness dir before `go run .`.
    let module = shared_module();
    let union_ids: std::collections::HashSet<String> = module
        .shapes
        .iter()
        .filter(|s| matches!(s.kind, tono_backend::ir::ShapeKind::Union { .. }))
        .map(|s| s.id.clone())
        .collect();
    for module_file in go::emit::emit_module(&module, &go::types::go_casing(), &union_ids) {
        let rough = render_file_with_companion(
            &module_file.file,
            module_file.imports_companion.as_deref(),
            &go::GoRules::default(),
            &Formatter::new("cat", vec![]),
        )
        .text;
        let source = format!("{}\n{}", go::emit::package_clause("main"), rough);
        let formatted = Formatter::new("gofmt", vec![]).run(&source);
        std::fs::write(
            dir.join(format!("models{}.go", module_file.suffix)),
            formatted.text,
        )
        .expect("write go source");
    }
    let out = run(
        &dir,
        "go",
        &["run", "-tags", "conformance", "."],
        Some(&batch_stdin(inputs)),
    );
    Some(parse_batch("go", inputs, &out))
}

/// The TypeScript conformance driver. The inputs are embedded (so the driver
/// needs no Node type declarations to read stdin); it decodes then re-encodes
/// each and prints one wire JSON per line.
fn ts_driver(inputs: &[Value]) -> String {
    let embedded = serde_json::to_string(inputs).expect("encode inputs");
    format!(
        "import {{ decodeAccount, encodeAccount }} from \"./models_serde\";\n\
         const inputs: any[] = {embedded};\n\
         for (const input of inputs) {{\n\
           console.log(JSON.stringify(encodeAccount(decodeAccount(input))));\n\
         }}\n"
    )
}

fn ts_outputs(inputs: &[Value], work_tag: &str) -> Option<Vec<Value>> {
    let ws = tests_dir().join("typescript");
    let tsc = ws.join("node_modules/.bin/tsc");
    if !tsc.exists() || !have("node", "--version") {
        return None;
    }
    let work_name = format!("work-{work_tag}");
    let work = ws.join(&work_name);
    std::fs::create_dir_all(&work).expect("create ts work dir");
    // TypeScript splits the module into a types file and a serde file; write both,
    // then compile both alongside the driver.
    for module_file in
        typescript::emit::emit_module(&shared_module(), &typescript::types::ts_casing())
    {
        let text = render_file_with_companion(
            &module_file.file,
            module_file.imports_companion.as_deref(),
            &typescript::TsRules,
            &Formatter::new("cat", vec![]),
        )
        .text;
        std::fs::write(work.join(format!("models{}.ts", module_file.suffix)), text)
            .expect("write ts models source");
    }
    std::fs::write(work.join("conformance.ts"), ts_driver(inputs)).expect("write conformance.ts");
    let compile = Command::new(&tsc)
        .args([
            &format!("{work_name}/models.ts"),
            &format!("{work_name}/models_serde.ts"),
            &format!("{work_name}/conformance.ts"),
            "--outDir",
            &format!("{work_name}/dist"),
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
    let out = run(
        &ws,
        "node",
        &[&format!("{work_name}/dist/conformance.js")],
        None,
    );
    Some(parse_batch("typescript", inputs, &out))
}

/// Run every port over the same batch of wire documents. Absent toolchains yield
/// `None`; `work_tag` keeps each harness's TypeScript scratch dir separate.
pub fn all_port_outputs(
    inputs: &[Value],
    work_tag: &str,
) -> Vec<(&'static str, Option<Vec<Value>>)> {
    vec![
        ("rust", rust_outputs(inputs)),
        ("go", go_outputs(inputs)),
        ("typescript", ts_outputs(inputs, work_tag)),
    ]
}

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
use tono_backend::codegen::layout::SameUnit;
use tono_backend::codegen::render::render_file_with;
use tono_backend::codegen::targets::{go, rust, typescript};
use tono_backend::codegen::visibility::Exposed;
use tono_backend::codegen::Formatter;
use tono_backend::codegen::{generate_target, resolve_groups, CodegenConfig, TargetKind};
use tono_backend::ir::Model;

mod common;
use common::{
    matrix_module as shared_module, wire_with_secret, BASE64_ACCEPTED, BASE64_REJECTED,
    CANONICAL_WIRE as CANONICAL,
};

/// The line a driver prints for a document its SDK refuses to decode.
const REJECT: &str = "REJECT";

/// Every document the drivers are fed, canonical first. The rest probe the one
/// field each target decodes by hand, which is where the three drifted apart.
fn documents() -> Vec<String> {
    let mut documents = vec![CANONICAL.to_string(), wire_with_secret(BASE64_ACCEPTED)];
    documents.extend(BASE64_REJECTED.iter().map(|s| wire_with_secret(s)));
    documents
}

/// The drivers' input: one JSON array of documents, so a language pays its
/// toolchain start-up once however many documents there are.
fn driver_input() -> String {
    format!("[{}]", documents().join(","))
}

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

/// Generate the whole SDK for a target and write the tree under `root`, so the
/// driver consumes it exactly as the layout emits it.
fn write_sdk(root: &Path, target: TargetKind, casing: &tono_backend::codegen::CasingConfig) {
    let model = Model {
        tono_ir_version: 6,
        modules: vec![shared_module()],
    };
    for file in
        generate_target(&model, target, &CodegenConfig::default(), casing).expect("generates")
    {
        let relative = file
            .path
            .strip_prefix(target.dir())
            .expect("target-rooted path");
        let out = root.join(relative);
        std::fs::create_dir_all(out.parent().expect("a parent")).expect("create module dir");
        std::fs::write(&out, &file.text).expect("write generated source");
    }
}

fn rust_output() -> Option<Vec<String>> {
    if !have("cargo", "--version") {
        return None;
    }
    let dir = tests_dir().join("rust");
    // The generated SDK is the harness crate's library; the driver is a binary
    // that consumes it from outside. Rust paths already carry the crate's
    // `src/` segment, so the stripped tree lands at the crate root.
    write_sdk(&dir, TargetKind::Rust, &rust::types::rust_casing());
    let out = run(
        &dir,
        "cargo",
        &["run", "--quiet", "--bin", "conformance"],
        Some(&driver_input()),
    );
    Some(lines(&out))
}

/// One line per document, in the order they were fed.
fn lines(out: &str) -> Vec<String> {
    out.lines().map(str::to_string).collect()
}

/// A driver's answers as data rather than bytes: `None` where it refused the
/// document, else the re-encoded JSON parsed, since key order carries no
/// meaning and the targets do not agree on it.
fn answers(lines: &[String]) -> Vec<Option<Value>> {
    lines
        .iter()
        .map(|line| {
            (line != REJECT)
                .then(|| serde_json::from_str(line).expect("a driver line is REJECT or json"))
        })
        .collect()
}

fn go_output() -> Option<Vec<String>> {
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
    let mut files = go::emit::emit_module(
        &module,
        &go::types::go_casing(),
        &union_ids,
        &Exposed::all(),
    );
    resolve_groups(&mut files, TargetKind::Go);
    for module_file in files {
        let rough = render_file_with(
            &module_file.file,
            &SameUnit {
                target: TargetKind::Go,
            },
            &go::GoRules {
                go_module: None,
                current: module_file.group.path(),
            },
            &Formatter::new("cat", vec![]),
        )
        .text;
        let source = format!("{}\n{}", go::emit::package_clause("main"), rough);
        let formatted = Formatter::new("gofmt", vec![]).run(&source);
        std::fs::write(
            dir.join(format!("models_{}.go", module_file.group.name)),
            formatted.text,
        )
        .expect("write go source");
    }
    let out = run(
        &dir,
        "go",
        &["run", "-tags", "conformance", "."],
        Some(&driver_input()),
    );
    Some(lines(&out))
}

/// The TypeScript conformance driver. The documents are embedded (so the driver
/// needs no Node type declarations to read stdin); it decodes then re-encodes
/// each and prints the wire JSON, or REJECT for one the SDK refuses.
fn ts_driver() -> String {
    format!(
        "import {{ decodeAccount, encodeAccount }} from \"./models/codec\";\n\
         const documents: any[] = {input};\n\
         for (const document of documents) {{\n\
         \x20 try {{\n\
         \x20   console.log(JSON.stringify(encodeAccount(decodeAccount(document))));\n\
         \x20 }} catch {{\n\
         \x20   console.log(\"{REJECT}\");\n\
         \x20 }}\n\
         }}\n",
        input = driver_input()
    )
}

fn ts_output() -> Option<Vec<String>> {
    let ws = tests_dir().join("typescript");
    let tsc = ws.join("node_modules/.bin/tsc");
    if !tsc.exists() || !have("node", "--version") {
        return None;
    }
    let work = ws.join("work-conformance");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("create work-conformance");
    write_sdk(
        &work,
        TargetKind::TypeScript,
        &typescript::types::ts_casing(),
    );
    std::fs::write(work.join("conformance.ts"), ts_driver()).expect("write conformance.ts");
    // Naming the sources on the command line makes tsc ignore any tsconfig in
    // scope, which it now rejects outright, so the settings live in a project
    // file of their own next to the round-trip one.
    let compile = Command::new(&tsc)
        .args(["-p", "tsconfig.conformance.json"])
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
    Some(lines(&out))
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

    let expected = documents();
    for (name, output) in &outputs {
        let Some(lines) = output else { continue };
        assert_eq!(
            lines.len(),
            expected.len(),
            "{name} answered {} of {} documents",
            lines.len(),
            expected.len()
        );
        // The canonical document round-trips to itself.
        assert_eq!(
            serde_json::from_str::<Value>(&lines[0]).expect("{name} output is json"),
            canonical,
            "{name} re-encoded JSON is not canonically equal to the fixture"
        );
        // The one unusual spelling every target accepts decodes to the same
        // bytes, so re-encoding it lands back on the canonical spelling.
        assert_ne!(
            lines[1], REJECT,
            "{name} refused a base64 spelling the contract accepts"
        );
        for (line, document) in lines[2..].iter().zip(&expected[2..]) {
            assert_eq!(
                line, REJECT,
                "{name} accepted a malformed document instead of refusing it: {document}"
            );
        }
    }
    // Beyond each target being right on its own, the three have to answer
    // identically: a divergence here is the wire contract splitting in two.
    let answered: Vec<(&str, Vec<Option<Value>>)> = outputs
        .iter()
        .filter_map(|(name, out)| out.as_ref().map(|lines| (*name, answers(lines))))
        .collect();
    for pair in answered.windows(2) {
        let [(a, left), (b, right)] = pair else {
            continue;
        };
        assert_eq!(left, right, "{a} and {b} do not agree on the wire");
    }
}

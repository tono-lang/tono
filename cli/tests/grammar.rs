//! The pinned tree-sitter grammar must parse everything the compiler accepts.
//!
//! The grammar lives in another repository and reaches this one only through
//! the rev pinned in `cli/Cargo.toml`. Nothing else in the build exercises it,
//! so a language change that lands without a pin bump used to surface only in
//! an editor, weeks later. This test walks every example the compiler compiles
//! and fails on the first file the pinned grammar cannot parse cleanly, naming
//! the file and where the error node sits.

#![cfg(feature = "preview")]

use std::fs;
use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

/// Every `.tono` file under `examples/`, sorted so a failure is stable.
fn example_sources() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples");
    let mut found = Vec::new();
    collect(&root, &mut found);
    found.sort();
    assert!(
        !found.is_empty(),
        "no .tono examples found under {}",
        root.display()
    );
    found
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "tono") {
            out.push(path);
        }
    }
}

/// The first ERROR or MISSING node in the tree, depth first, as
/// `line:col kind` plus the offending source line.
fn first_error(node: Node, source: &str) -> Option<String> {
    if node.is_error() || node.is_missing() {
        let start = node.start_position();
        let line = source.lines().nth(start.row).unwrap_or("");
        let kind = if node.is_missing() {
            "missing"
        } else {
            "error"
        };
        return Some(format!(
            "{}:{} {kind} node `{}` in: {}",
            start.row + 1,
            start.column + 1,
            node.kind(),
            line.trim()
        ));
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = first_error(child, source) {
            return Some(found);
        }
    }
    None
}

fn parse_failures(paths: &[PathBuf]) -> Vec<String> {
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_tono::LANGUAGE.into())
        .expect("the pinned tono grammar loads");
    let mut failures = Vec::new();
    for path in paths {
        let source =
            fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        let tree = parser
            .parse(&source, None)
            .expect("parsing never times out");
        if let Some(problem) = first_error(tree.root_node(), &source) {
            let shown = path
                .strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."))
                .unwrap_or(path);
            failures.push(format!("{}: {problem}", shown.display()));
        }
    }
    failures
}

#[test]
fn pinned_grammar_parses_every_example() {
    let failures = parse_failures(&example_sources());
    assert!(
        failures.is_empty(),
        "the pinned tree-sitter-tono grammar rejects {} example(s); bump the rev in \
         cli/Cargo.toml to a grammar that knows this syntax:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn a_syntax_error_is_reported_with_its_location() {
    let dir = std::env::temp_dir().join(format!("tono-grammar-gate-{}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch dir");
    let file = dir.join("broken.tono");
    fs::write(&file, "pub struct A {\n  id: string\n}\nstruct {\n").expect("write fixture");
    let failures = parse_failures(std::slice::from_ref(&file));
    fs::remove_dir_all(&dir).ok();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert!(
        failures[0].contains("broken.tono: 4:"),
        "names the file and line: {}",
        failures[0]
    );
}

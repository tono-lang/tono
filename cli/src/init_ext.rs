//! `tono init`'s best-effort scan for `ext` blocks, so a fresh or updated
//! manifest can scaffold a commented `[ext.<name>]` section instead of
//! leaving the user to discover the requirement from a failed `tono gen`.
//!
//! This is a text scan, not a parse: `init` has no frontend/IR access, and
//! getting the language keys wrong here only means a slightly wrong comment,
//! not a build break. The real gate is `tono gen`'s validation against the
//! compiled model. A block whose opening `{` is not on the same line as
//! `ext <name>` is missed.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use tono_backend::config::normalize_ext_lang;

/// Every `ext <name> { ... }` block found under `root`, mapped to the
/// language keys (`go`/`rust`/`ts`) it declares a module path for.
pub fn scan_ext_names(root: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut found = BTreeMap::new();
    for path in tono_files(root) {
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        scan_text(&text, &mut found);
    }
    found
}

fn scan_text(text: &str, found: &mut BTreeMap<String, BTreeSet<String>>) {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let Some(rest) = trimmed.strip_prefix("ext ") else {
            continue;
        };
        let Some(name) = rest.split_whitespace().next() else {
            continue;
        };
        if !rest.contains('{') {
            continue;
        }
        let langs = found.entry(name.to_string()).or_default();
        let mut depth = brace_delta(line);
        if depth <= 0 {
            continue;
        }
        for body_line in lines.by_ref() {
            depth += brace_delta(body_line);
            record_lang(body_line, langs);
            if depth <= 0 {
                break;
            }
        }
    }
}

fn brace_delta(line: &str) -> i32 {
    line.matches('{').count() as i32 - line.matches('}').count() as i32
}

fn record_lang(line: &str, langs: &mut BTreeSet<String>) {
    let trimmed = line.trim_start();
    for key in ["go", "rust", "ts", "typescript"] {
        if let Some(rest) = trimmed.strip_prefix(key) {
            if rest.trim_start().starts_with(':') {
                langs.insert(normalize_ext_lang(key).to_string());
            }
        }
    }
}

/// Every `.tono` file under `root`, recursively. Manual recursion (the same
/// technique `gen.rs::sweep_dir` uses) so this needs no directory-walking
/// dependency.
fn tono_files(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    collect_tono_files(root, &mut files);
    files
}

fn collect_tono_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tono_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "tono") {
            out.push(path);
        }
    }
}

/// The commented `[ext.<name>]` block `tono init` scaffolds: a placeholder
/// version for every language the block declared, so filling it in is a
/// one-line edit per language.
pub fn ext_block(name: &str, langs: &BTreeSet<String>) -> String {
    let mut out = format!(
        "\n# Pin the version of \"{name}\" for each target that uses it.\n\
         # [ext.{name}]\n"
    );
    for lang in langs {
        out.push_str(&format!("# {lang} = \"\"\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_simple_ext_block() {
        let src = "ext companyconfig {\n  go: \"github.com/company/config\"\n  ts: \"@company/config\"\n}\n";
        let found = {
            let mut m = BTreeMap::new();
            scan_text(src, &mut m);
            m
        };
        let langs = found.get("companyconfig").unwrap();
        assert!(langs.contains("go"));
        assert!(langs.contains("ts"));
    }

    #[test]
    fn typescript_key_normalizes_to_ts() {
        let src = "ext x {\n  typescript: \"@x/y\"\n}\n";
        let mut m = BTreeMap::new();
        scan_text(src, &mut m);
        assert!(m.get("x").unwrap().contains("ts"));
    }

    #[test]
    fn ignores_nested_braces_from_struct_bodies() {
        let src = "ext companyconfig {\n  go: \"github.com/company/config\"\n\n  struct go_config { Host: string }\n}\n\next companybus {\n  go: \"github.com/company/bus\"\n}\n";
        let mut m = BTreeMap::new();
        scan_text(src, &mut m);
        assert!(m.contains_key("companyconfig"));
        assert!(m.contains_key("companybus"));
        assert!(m["companyconfig"].contains("go"));
        assert!(m["companybus"].contains("go"));
    }

    #[test]
    fn no_ext_blocks_is_empty() {
        let mut m = BTreeMap::new();
        scan_text("pub struct client { name: string }\n", &mut m);
        assert!(m.is_empty());
    }

    #[test]
    fn a_block_whose_brace_is_not_on_the_same_line_is_missed() {
        // Documented limitation: the scan only recognizes `ext <name> {` with
        // the opening brace on the same line.
        let mut m = BTreeMap::new();
        scan_text(
            "ext companyconfig\n{\n  go: \"github.com/company/config\"\n}\n",
            &mut m,
        );
        assert!(m.is_empty());
    }

    #[test]
    fn a_block_closed_on_the_same_line_records_the_name_with_no_langs() {
        let mut m = BTreeMap::new();
        scan_text("ext empty { }\n", &mut m);
        let langs = m.get("empty").expect("the name is still recorded");
        assert!(langs.is_empty());
    }

    #[test]
    fn a_bare_ext_keyword_with_nothing_after_it_is_ignored() {
        let mut m = BTreeMap::new();
        scan_text("ext \n", &mut m);
        assert!(m.is_empty());
    }

    #[test]
    fn scan_ext_names_on_a_missing_root_is_empty() {
        let found = scan_ext_names(Path::new("/no/such/tono-init-ext-test-root"));
        assert!(found.is_empty());
    }
}

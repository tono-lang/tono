//! The Rust extractor: the crate's source located through `cargo metadata`
//! of the consumer's own manifest (so the version, the registry checkout
//! or the `[patch]` the SDK builds against is the one read), then parsed
//! with syn and reduced to its public API.
//!
//! rustdoc's JSON output would be the compiler's own view, but it is
//! nightly-only; a syntactic read works on every toolchain, with one edge
//! the note records: an API a macro produces is not in the source.

use std::path::{Path, PathBuf};
use std::process::Command;

use super::{rust_exports, rust_walk, Outcome};

/// The crate's library root and a note, from cargo's resolved graph: the
/// package whose name is `spelling` (`-` and `_` read alike), at `version`
/// when the graph holds several.
pub(crate) fn locate_package(
    metadata: &serde_json::Value,
    spelling: &str,
    version: &str,
) -> Result<(PathBuf, Option<String>), String> {
    let wanted = spelling.replace('-', "_");
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    let mut candidates: Vec<&serde_json::Value> = packages
        .iter()
        .filter(|p| {
            p["name"]
                .as_str()
                .is_some_and(|n| n.replace('-', "_") == wanted)
        })
        .collect();
    if candidates.is_empty() {
        return Err(format!(
            "crate {spelling} is not in the dependency graph (is it in Cargo.toml?)"
        ));
    }
    let mut note = None;
    let chosen = if let Some(exact) = candidates
        .iter()
        .find(|p| p["version"].as_str() == Some(version))
    {
        exact
    } else if candidates.len() == 1 {
        candidates[0]
    } else {
        candidates.sort_by_key(|p| p["version"].as_str().unwrap_or_default().to_string());
        let last = candidates.last().copied().unwrap();
        note = Some(format!(
            "several versions of {spelling} in the graph; indexed {}",
            last["version"].as_str().unwrap_or_default()
        ));
        last
    };
    let targets = chosen["targets"].as_array().cloned().unwrap_or_default();
    let is_lib = |t: &serde_json::Value| {
        t["kind"].as_array().is_some_and(|kinds| {
            kinds
                .iter()
                .any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "dylib")))
        })
    };
    if targets.iter().any(|t| {
        t["kind"]
            .as_array()
            .is_some_and(|k| k.iter().any(|k| k == "proc-macro"))
    }) && !targets.iter().any(is_lib)
    {
        return Err(format!(
            "crate {spelling} is a proc-macro crate; its API is what the macros produce"
        ));
    }
    let src = targets
        .iter()
        .find(|t| is_lib(t))
        .and_then(|t| t["src_path"].as_str())
        .ok_or_else(|| format!("crate {spelling} has no library target"))?;
    Ok((PathBuf::from(src), note))
}

fn cargo_metadata(manifest: &Path) -> Result<serde_json::Value, String> {
    let run = |offline: bool| {
        let mut cmd = Command::new("cargo");
        cmd.args(["metadata", "--format-version", "1", "--manifest-path"])
            .arg(manifest);
        if offline {
            cmd.arg("--offline");
        }
        cmd.output()
    };
    // Offline first: the SDK's dependencies are usually fetched already,
    // and a network round trip is not what an editor index should wait on.
    // A graph offline cannot resolve is tried once with the network.
    let output = match run(true) {
        Ok(o) if o.status.success() => o,
        Ok(_) => run(false).map_err(|e| e.to_string())?,
        Err(_) => return Err("cargo is not installed".to_string()),
    };
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())
            .unwrap_or("cargo metadata failed");
        return Err(format!("cargo metadata: {last}"));
    }
    serde_json::from_slice(&output.stdout).map_err(|e| format!("cargo metadata: {e}"))
}

/// The symbols of crate `spelling` as the `Cargo.toml` at `root` resolves
/// it. Every reason nothing can be read is a skip, never an error: the
/// consumer's manifest may not exist yet, cargo may be absent, the crate
/// may be a proc-macro.
pub(crate) fn extract(root: &Path, spelling: &str, version: &str) -> Result<Outcome, String> {
    let manifest = root.join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(Outcome::Skipped(format!(
            "no Cargo.toml at {} (run tono gen first)",
            root.display()
        )));
    }
    let metadata = match cargo_metadata(&manifest) {
        Ok(m) => m,
        Err(reason) => return Ok(Outcome::Skipped(reason)),
    };
    let (lib_rs, mut notes) = match locate_package(&metadata, spelling, version) {
        Ok((path, note)) => (path, note.into_iter().collect::<Vec<_>>()),
        Err(reason) => return Ok(Outcome::Skipped(reason)),
    };
    let tree = match rust_walk::load_crate(&lib_rs) {
        Ok(t) => t,
        Err(reason) => return Ok(Outcome::Skipped(reason)),
    };
    let (symbols, read_notes) = rust_exports::public_api(&tree);
    notes.extend(read_notes);
    notes.push("parsed from source: what a macro produces is not indexed".to_string());
    Ok(Outcome::Built {
        symbols,
        note: Some(notes.join("; ")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::SymbolKind;

    fn metadata(packages: &str) -> serde_json::Value {
        serde_json::from_str(&format!(r#"{{"packages":[{packages}]}}"#)).unwrap()
    }

    const GEARBOX_1: &str = r#"{"name":"gear-box","version":"1.0.0","targets":[{"kind":["lib"],"src_path":"/reg/gear-box-1.0.0/src/lib.rs"}]}"#;
    const GEARBOX_2: &str = r#"{"name":"gear-box","version":"2.0.0","targets":[{"kind":["rlib"],"src_path":"/reg/gear-box-2.0.0/src/lib.rs"}]}"#;

    #[test]
    fn the_package_is_found_by_name_and_version() {
        let (path, note) = locate_package(&metadata(GEARBOX_1), "gear_box", "1.0.0").unwrap();
        assert_eq!(path, PathBuf::from("/reg/gear-box-1.0.0/src/lib.rs"));
        assert_eq!(note, None);
        let both = metadata(&format!("{GEARBOX_1},{GEARBOX_2}"));
        let (path, note) = locate_package(&both, "gear-box", "2.0.0").unwrap();
        assert!(path.to_string_lossy().contains("2.0.0"));
        assert_eq!(note, None);
        // No exact match among several: the newest, said so.
        let (path, note) = locate_package(&both, "gear-box", "^1").unwrap();
        assert!(path.to_string_lossy().contains("2.0.0"));
        assert!(note.unwrap().contains("several versions"));
        // A single candidate wins whatever the version constraint spells.
        let (path, _) = locate_package(&metadata(GEARBOX_1), "gear-box", "^1").unwrap();
        assert!(path.to_string_lossy().contains("1.0.0"));
    }

    #[test]
    fn a_missing_crate_a_proc_macro_and_a_binary_only_crate_are_refused() {
        let err = locate_package(&metadata(GEARBOX_1), "other", "1").unwrap_err();
        assert!(err.contains("not in the dependency graph"), "{err}");
        let pm = metadata(
            r#"{"name":"derive-gear","version":"1.0.0","targets":[{"kind":["proc-macro"],"src_path":"/x/src/lib.rs"}]}"#,
        );
        let err = locate_package(&pm, "derive-gear", "1.0.0").unwrap_err();
        assert!(err.contains("proc-macro"), "{err}");
        let bin = metadata(
            r#"{"name":"tool","version":"1.0.0","targets":[{"kind":["bin"],"src_path":"/x/src/main.rs"}]}"#,
        );
        let err = locate_package(&bin, "tool", "1.0.0").unwrap_err();
        assert!(err.contains("no library target"), "{err}");
    }

    #[test]
    fn a_manifest_cargo_cannot_read_is_skipped_with_its_last_line() {
        if Command::new("cargo").arg("--version").output().is_err() {
            eprintln!("skipping: cargo is not installed");
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "tono-index-rust-{}-badmanifest",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nversion = \"0.0.0\"\n").unwrap();
        match extract(&dir, "gearbox", "0.0.0").unwrap() {
            Outcome::Skipped(reason) => assert!(reason.starts_with("cargo metadata:"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_a_cargo_manifest_at_the_root_the_pair_is_skipped() {
        let dir =
            std::env::temp_dir().join(format!("tono-index-rust-{}-nomanifest", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        match extract(&dir, "gearbox", "0.0.0").unwrap() {
            Outcome::Skipped(reason) => assert!(reason.contains("no Cargo.toml"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_crate_is_read_through_the_consumer_manifest() {
        if Command::new("cargo").arg("--version").output().is_err() {
            eprintln!("skipping: cargo is not installed");
            return;
        }
        let lib = crate::index::rust_walk::tests::fixture_crate("consumer-lib");
        std::fs::write(
            lib.join("Cargo.toml"),
            "[package]\nname = \"gear-box\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
        )
        .unwrap();
        let root =
            std::env::temp_dir().join(format!("tono-index-rust-{}-consumer", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"sdk\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ngear-box = {{ path = {:?} }}\n\n[workspace]\n",
                lib.display()
            ),
        )
        .unwrap();
        let (symbols, note) = match extract(&root, "gear_box", "0.0.0").unwrap() {
            Outcome::Built { symbols, note } => (symbols, note),
            Outcome::Skipped(reason) => panic!("skipped: {reason}"),
        };
        let note = note.unwrap();
        assert!(
            note.ends_with("what a macro produces is not indexed"),
            "{note}"
        );
        let open = symbols.iter().find(|s| s.name == "open").unwrap();
        assert_eq!(open.kind, SymbolKind::Function);
        assert!(symbols.iter().any(|s| s.name == "Hidden"));
        assert!(root.join("Cargo.lock").is_file());
        match extract(&root, "nowhere", "0.0.0").unwrap() {
            Outcome::Skipped(reason) => {
                assert!(reason.contains("not in the dependency graph"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }
}

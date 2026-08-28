//! The Go extractor's Rust side: where the helper runs and how its report
//! comes back. The reading itself is `helpers/extract.go`, compiled and run
//! by the consumer's own `go` inside their module.

use std::path::Path;

use tono_backend::codegen::verify::{go_module_of, Scratch};

use super::{run_helper, Outcome};

const HELPER: &str = include_str!("helpers/extract.go");

/// The symbols of the package at `import_path`, resolved from the Go module
/// `root` sits in. The helper is a `main` package written into a scratch
/// directory of that module, so `go run` resolves the library through the
/// consumer's `go.mod` (its `require` and `replace` directives included).
pub(crate) fn extract(root: &Path, import_path: &str) -> Result<Outcome, String> {
    let (_, module_dir) = match go_module_of(root) {
        Ok(found) => found,
        Err(reason) => return Ok(Outcome::Skipped(reason)),
    };
    let scratch = Scratch::create(&module_dir, "index-go").map_err(|e| e.to_string())?;
    std::fs::write(scratch.dir.join("main.go"), HELPER).map_err(|e| e.to_string())?;
    run_helper(
        "go",
        &["run", ".", import_path],
        &scratch.dir,
        "go is not installed",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tono-index-go-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn without_a_go_module_above_the_root_the_pair_is_skipped() {
        let dir = tmp("nomod");
        match extract(&dir, "example.test/gearbox").unwrap() {
            Outcome::Skipped(reason) => assert!(reason.contains("no go.mod above"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    /// With a module but no such dependency, the helper (when `go` is
    /// installed) or the missing toolchain (when it is not) is a skip
    /// naming the cause; the scratch directory is gone either way. The
    /// helper's reading of a real package is covered end to end by the
    /// `index` integration suite.
    #[test]
    fn a_package_the_module_does_not_require_is_skipped_and_the_scratch_removed() {
        let dir = tmp("unknown");
        std::fs::write(
            dir.join("go.mod"),
            "module example.test/consumer\n\ngo 1.21\n",
        )
        .unwrap();
        match extract(&dir, "example.test/nowhere").unwrap() {
            Outcome::Skipped(reason) => assert!(
                reason.contains("nowhere") || reason.contains("go is not installed"),
                "{reason}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")));
    }
}

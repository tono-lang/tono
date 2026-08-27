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
    use crate::index::{MemberKind, SymbolKind};

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tono-index-go-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn without_a_go_module_above_the_root_the_pair_is_skipped() {
        let dir = tmp("nomod");
        match extract(&dir, "example.test/gearbox").unwrap() {
            Outcome::Skipped(reason) => assert!(reason.contains("no go.mod above"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_helper_reads_a_stand_in_package_through_the_consumer_module() {
        if std::process::Command::new("go")
            .arg("version")
            .output()
            .is_err()
        {
            eprintln!("skipping: go is not installed");
            return;
        }
        let dir = tmp("lib");
        let lib = dir.join("gearbox-go");
        write(
            &lib.join("go.mod"),
            "module example.test/gearbox\n\ngo 1.21\n",
        );
        write(
            &lib.join("gearbox.go"),
            "package gearbox\n\nimport \"context\"\n\n\
             // Dial reads a value.\n\
             type Dial[T any] interface{ Read(ctx context.Context) (T, error) }\n\n\
             type Options struct { Name string; hidden int }\n\n\
             func (o *Options) Apply() {}\n\n\
             type Mode int\n\n\
             const Fast Mode = 1\n\n\
             var Version = \"1\"\n\n\
             func Open[T any](value T, opts ...Options) (Dial[T], error) { return nil, nil }\n\n\
             func internal() {}\n",
        );
        let root = dir.join("consumer-go");
        write(
            &root.join("go.mod"),
            &format!(
                "module example.test/consumer\n\ngo 1.21\n\nrequire example.test/gearbox v0.0.0\n\nreplace example.test/gearbox => {}\n",
                lib.display()
            ),
        );
        let (symbols, note) = match extract(&root, "example.test/gearbox").unwrap() {
            Outcome::Built { symbols, note } => (symbols, note),
            Outcome::Skipped(reason) => panic!("skipped: {reason}"),
        };
        assert!(note.unwrap().contains("no documentation"));
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Dial", "Fast", "Mode", "Open", "Options", "Version"]
        );
        let open = &symbols[3];
        assert_eq!(open.kind, SymbolKind::Function);
        assert_eq!(
            open.signatures,
            vec!["func[T any](value T, opts ...Options) (Dial[T], error)"]
        );
        let dial = &symbols[0];
        assert_eq!(dial.kind, SymbolKind::Interface);
        assert_eq!(dial.signatures, vec!["Dial[T any]"]);
        assert_eq!(dial.members[0].name, "Read");
        assert_eq!(dial.members[0].kind, MemberKind::Method);
        assert_eq!(
            dial.members[0].signatures,
            vec!["func(ctx context.Context) (T, error)"]
        );
        let options = &symbols[4];
        assert_eq!(options.kind, SymbolKind::Struct);
        let members: Vec<(&str, MemberKind)> = options
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.kind))
            .collect();
        assert_eq!(
            members,
            vec![("Name", MemberKind::Field), ("Apply", MemberKind::Method)]
        );
        assert_eq!(symbols[1].signatures, vec!["Mode = 1"]);
        assert_eq!(symbols[2].kind, SymbolKind::Type);
        // The scratch directory is gone with the extractor.
        assert!(!std::fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")));
    }

    #[test]
    fn a_package_the_module_does_not_require_is_skipped_with_the_reason() {
        if std::process::Command::new("go")
            .arg("version")
            .output()
            .is_err()
        {
            eprintln!("skipping: go is not installed");
            return;
        }
        let dir = tmp("unknown");
        write(
            &dir.join("go.mod"),
            "module example.test/consumer\n\ngo 1.21\n",
        );
        match extract(&dir, "example.test/nowhere").unwrap() {
            Outcome::Skipped(reason) => assert!(reason.contains("nowhere"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}

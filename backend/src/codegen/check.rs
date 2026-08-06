//! The native compile-check: prove the generated SDK is not just rendered but
//! valid, by building it with the target language's own toolchain.
//!
//! The engine emits source; this module scaffolds a throwaway project around that
//! source (the manifest and crate/module roots a consumer would otherwise write by
//! hand), then runs the real compiler (`cargo`, `go`, `tsc`) as a subprocess and
//! maps its exit to a verdict. Like the [`Formatter`](super::format::Formatter),
//! it must never fail the caller: a missing toolchain degrades to a distinct
//! [`CheckOutcome::ToolchainMissing`] rather than an error, so the preview works
//! without every compiler installed, just with fewer verdicts.
//!
//! The scratch directory is the caller's to own. Reusing one across checks lets
//! each toolchain keep its incremental build cache (a re-check after a small edit
//! is a fast rebuild, not a cold one), which is what makes a live preview usable.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::codegen::pipeline::{GeneratedFile, TargetKind};

/// The verdict of a compile-check: whether the target toolchain accepted the
/// generated source, and why not when it did not. The two failure cases are kept
/// distinct because they mean different things: a rejected build is a signal about
/// the generated code, while a missing toolchain is an environment gap the caller
/// should surface as a warning rather than a red verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    /// The toolchain ran and accepted the generated source.
    Compiles,
    /// The toolchain ran and rejected the source; carries its diagnostics.
    Failed { diagnostics: String },
    /// The toolchain binary could not be spawned (not installed, or not on
    /// `PATH`).
    ToolchainMissing { program: String },
}

/// Scaffold a throwaway project for `target` around `files` in `scratch`, then
/// build it with the target's toolchain and return the verdict. `scratch` is
/// created if missing and is the caller's to reuse (for cache) or discard.
///
/// The `io::Result` covers only the scaffolding filesystem work; a compiler that
/// runs and rejects the source, or one that is not installed, is a
/// [`CheckOutcome`], never an `Err`.
pub fn check(
    target: TargetKind,
    files: &[GeneratedFile],
    scratch: &Path,
) -> io::Result<CheckOutcome> {
    let plan = scaffold(target, files, scratch)?;
    Ok(run_checker(&plan.program, &plan.args, &plan.cwd))
}

/// A ready-to-run compiler invocation: the program, its arguments, and the
/// directory to run it in. Produced by [`scaffold`] once the project is on disk.
struct CheckPlan {
    program: String,
    args: Vec<String>,
    cwd: PathBuf,
}

/// Write `files` (and the manifest and roots the toolchain needs) into `scratch`
/// laid out as an idiomatic project for `target`, returning how to build it. The
/// generated paths are rooted at the target directory (`rust/…`, `go/…`,
/// `typescript/…`); that first segment is the SDK's place in a consumer tree, not
/// part of this throwaway project, so it is stripped here.
fn scaffold(target: TargetKind, files: &[GeneratedFile], scratch: &Path) -> io::Result<CheckPlan> {
    match target {
        TargetKind::Rust => scaffold_rust(files, scratch),
        TargetKind::Go => scaffold_go(files, scratch),
        TargetKind::TypeScript => scaffold_typescript(files, scratch),
    }
}

/// A crate with the generated modules under `src/` and a `lib.rs` that declares
/// each top-level one, built with `cargo build`. serde is a dependency because the
/// generated types derive it.
fn scaffold_rust(files: &[GeneratedFile], scratch: &Path) -> io::Result<CheckPlan> {
    let src = scratch.join("src");
    write_sources(files, TargetKind::Rust, &src)?;

    // The crate root declares each top-level generated module (a bare file, or a
    // directory carrying its own `mod.rs`); anything nested is declared by those.
    let mut lib = String::new();
    for name in top_level_rust_modules(&src)? {
        lib.push_str(&format!("pub mod {name};\n"));
    }
    fs::write(src.join("lib.rs"), lib)?;

    // reqwest (default-on feature) and tokio mirror the manifest a consumer
    // embeds a generated entry client with; a model with no entry simply
    // leaves them unused.
    fs::write(
        scratch.join("Cargo.toml"),
        "[package]\n\
         name = \"tono_preview\"\n\
         version = \"0.0.0\"\n\
         edition = \"2021\"\n\
         [features]\n\
         default = [\"reqwest\"]\n\
         reqwest = [\"dep:reqwest\"]\n\
         [dependencies]\n\
         serde = { version = \"1\", features = [\"derive\"] }\n\
         serde_json = \"1\"\n\
         reqwest = { version = \"0.12\", default-features = false, features = [\"rustls-tls\"], optional = true }\n\
         tokio = { version = \"1\", features = [\"time\"] }\n\
         [workspace]\n",
    )?;

    Ok(CheckPlan {
        program: "cargo".into(),
        args: vec!["check".into(), "--quiet".into()],
        cwd: scratch.to_path_buf(),
    })
}

/// The generated `.rs` entries directly under `src`, sorted, each reduced to its
/// module name: a directory is a module, a `foo.rs` file is module `foo`, and
/// `lib.rs` is the root we are generating (skipped).
fn top_level_rust_modules(src: &Path) -> io::Result<Vec<String>> {
    let mut modules = Vec::new();
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() {
            modules.push(name);
        } else if let Some(stem) = name.strip_suffix(".rs") {
            if stem != "lib" {
                modules.push(stem.to_string());
            }
        }
    }
    modules.sort();
    Ok(modules)
}

/// The module path the Go scaffold declares. A preview is not a real project, so
/// the path is fixed; generation has to be given the same one, since Go spells
/// every cross-package import with it.
pub const GO_SCAFFOLD_MODULE: &str = "tono_preview";

/// A single Go module holding the generated packages, built with `go build`.
fn scaffold_go(files: &[GeneratedFile], scratch: &Path) -> io::Result<CheckPlan> {
    write_sources(files, TargetKind::Go, scratch)?;
    fs::write(
        scratch.join("go.mod"),
        format!("module {GO_SCAFFOLD_MODULE}\n\ngo 1.21\n"),
    )?;
    Ok(CheckPlan {
        program: "go".into(),
        args: vec!["build".into(), "./...".into()],
        cwd: scratch.to_path_buf(),
    })
}

/// The generated modules under a tsconfig that type-checks without emitting.
fn scaffold_typescript(files: &[GeneratedFile], scratch: &Path) -> io::Result<CheckPlan> {
    write_sources(files, TargetKind::TypeScript, scratch)?;

    fs::write(
        scratch.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\n    \
         \"strict\": true,\n    \
         \"noEmit\": true,\n    \
         \"target\": \"ES2020\",\n    \
         \"module\": \"ES2022\",\n    \
         \"moduleResolution\": \"bundler\",\n    \
         \"lib\": [\"ES2020\", \"DOM\"],\n    \
         \"skipLibCheck\": true\n  },\n  \
         \"include\": [\"**/*.ts\"]\n}\n",
    )?;

    Ok(CheckPlan {
        program: "tsc".into(),
        args: vec!["-p".into(), "tsconfig.json".into()],
        cwd: scratch.to_path_buf(),
    })
}

/// Write each generated file for `target` under `root`, stripping the leading
/// target-directory segment (`rust/`, `go/`, `typescript/`) that only places the
/// SDK in a consumer tree. Parent directories are created as needed.
fn write_sources(files: &[GeneratedFile], target: TargetKind, root: &Path) -> io::Result<()> {
    for file in files.iter().filter(|f| f.target == target) {
        let relative = file.path.strip_prefix(target.dir()).unwrap_or(&file.path);
        let dest = root.join(relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dest, &file.text)?;
    }
    Ok(())
}

/// Spawn the checker in `cwd` and map its exit to a verdict: success compiles, a
/// non-zero exit is a rejection carrying the diagnostics (both streams, since Go
/// and Rust report on stderr while `tsc` reports on stdout), and a spawn failure
/// is a missing toolchain. stdin is closed so a checker that reads it sees EOF
/// rather than blocking.
fn run_checker(program: &str, args: &[String], cwd: &Path) -> CheckOutcome {
    let spawned = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let child = match spawned {
        Ok(child) => child,
        Err(_) => {
            return CheckOutcome::ToolchainMissing {
                program: program.to_string(),
            }
        }
    };
    match child.wait_with_output() {
        Ok(out) if out.status.success() => CheckOutcome::Compiles,
        Ok(out) => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            CheckOutcome::Failed {
                diagnostics: format!("{stdout}{stderr}").trim().to_string(),
            }
        }
        // Spawned but could not be waited on: treat as missing rather than a
        // rejection, since we learned nothing about the source.
        Err(_) => CheckOutcome::ToolchainMissing {
            program: program.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory unique to this test run, cleaned first so a rerun after
    /// a crash starts fresh. Mirrors the CLI tests' temp-dir convention (no
    /// tempfile dependency).
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tono-check-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn file(target: TargetKind, path: &str, text: &str) -> GeneratedFile {
        GeneratedFile {
            target,
            path: PathBuf::from(path),
            text: text.to_string(),
        }
    }

    #[test]
    fn rust_scaffold_lays_out_a_crate_and_declares_each_module() {
        let dir = scratch("rust-scaffold");
        let files = vec![
            file(TargetKind::Rust, "rust/payments.rs", "pub struct Charge;\n"),
            file(TargetKind::Rust, "rust/payments_serde.rs", "// serde\n"),
        ];
        let plan = scaffold_rust(&files, &dir).expect("scaffold");

        // Sources land under src/ with the `rust/` prefix stripped.
        assert_eq!(
            fs::read_to_string(dir.join("src/payments.rs")).unwrap(),
            "pub struct Charge;\n"
        );
        assert!(dir.join("src/payments_serde.rs").exists());
        // The crate root declares both top-level modules, sorted.
        let lib = fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert_eq!(lib, "pub mod payments;\npub mod payments_serde;\n");
        // A manifest with the serde dependency the generated types derive.
        let manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(manifest.contains("serde = { version = \"1\""));
        assert_eq!(plan.program, "cargo");
        assert_eq!(plan.args, vec!["check".to_string(), "--quiet".to_string()]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rust_scaffold_declares_a_nested_module_directory() {
        let dir = scratch("rust-nested");
        let files = vec![file(
            TargetKind::Rust,
            "rust/payments/common.rs",
            "pub struct Card;\n",
        )];
        scaffold_rust(&files, &dir).expect("scaffold");
        // A dotted module nests under a directory; the crate root declares the
        // directory, not the file inside it.
        assert!(dir.join("src/payments/common.rs").exists());
        let lib = fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert_eq!(lib, "pub mod payments;\n");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn go_scaffold_writes_the_module_and_sources() {
        let dir = scratch("go-scaffold");
        let files = vec![file(TargetKind::Go, "go/payments.go", "package payments\n")];
        let plan = scaffold_go(&files, &dir).expect("scaffold");
        assert_eq!(
            fs::read_to_string(dir.join("payments.go")).unwrap(),
            "package payments\n"
        );
        assert!(fs::read_to_string(dir.join("go.mod"))
            .unwrap()
            .starts_with("module tono_preview"));
        assert_eq!(plan.program, "go");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn typescript_scaffold_writes_a_plain_tsconfig() {
        let dir = scratch("ts-scaffold");
        let files = vec![file(
            TargetKind::TypeScript,
            "typescript/payments.ts",
            "export interface Charge {}\n",
        )];
        let plan = scaffold_typescript(&files, &dir).expect("scaffold");
        assert!(dir.join("payments.ts").exists());
        let tsconfig = fs::read_to_string(dir.join("tsconfig.json")).unwrap();
        assert!(!tsconfig.contains("paths"));
        assert!(!tsconfig.contains("baseUrl"));
        assert_eq!(plan.program, "tsc");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_sources_only_writes_the_requested_target() {
        let dir = scratch("write-filter");
        let files = vec![
            file(TargetKind::Rust, "rust/a.rs", "rust\n"),
            file(TargetKind::Go, "go/a.go", "go\n"),
        ];
        write_sources(&files, TargetKind::Rust, &dir).expect("write");
        assert!(dir.join("a.rs").exists());
        assert!(!dir.join("a.go").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_toolchain_is_reported_distinctly() {
        let dir = scratch("missing");
        fs::create_dir_all(&dir).unwrap();
        let outcome = run_checker("tono-no-such-compiler-xyz", &[], &dir);
        assert_eq!(
            outcome,
            CheckOutcome::ToolchainMissing {
                program: "tono-no-such-compiler-xyz".into()
            }
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_zero_exit_compiles() {
        // `true` exits 0 without touching the sources: it exercises the success
        // mapping deterministically, standing in for a compiler that accepts.
        let dir = scratch("ok");
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(run_checker("true", &[], &dir), CheckOutcome::Compiles);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nonzero_exit_is_a_rejection() {
        // `false` exits 1: it stands in for a compiler that rejects the source.
        let dir = scratch("reject");
        fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            run_checker("false", &[], &dir),
            CheckOutcome::Failed { .. }
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rejection_carries_the_combined_diagnostics() {
        // `sh -c 'echo out; echo err 1>&2; exit 1'` writes to both streams and
        // fails, proving both are captured into the diagnostics.
        let dir = scratch("diag");
        fs::create_dir_all(&dir).unwrap();
        let outcome = run_checker(
            "sh",
            &[
                "-c".into(),
                "echo on-stdout; echo on-stderr 1>&2; exit 1".into(),
            ],
            &dir,
        );
        match outcome {
            CheckOutcome::Failed { diagnostics } => {
                assert!(diagnostics.contains("on-stdout"));
                assert!(diagnostics.contains("on-stderr"));
            }
            other => panic!("expected a rejection, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }
}

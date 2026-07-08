//! Preview compile-check: drop the generated SDK into a minimal per-target
//! project and run that language's checker (`cargo check`, `go build`,
//! `tsc --noEmit`) to prove the emitted structure is valid. This is the
//! "compiles?" half of preview; rendering the source is the caller's job.
//!
//! The check is structural: no hand-written driver references the generated
//! symbols, so it works for any IR. A missing toolchain is not a failure, it is
//! reported as skipped, so preview degrades cleanly on a machine that lacks the
//! target's compiler.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::TargetKind;

/// The result of attempting a compile-check.
pub struct Outcome {
    /// Whether the toolchain was present and actually ran.
    pub ran: bool,
    /// Whether the generated code compiled (only meaningful when `ran`).
    pub ok: bool,
    /// The checker's tool name, for the report.
    pub tool: String,
    /// Combined stdout+stderr of the checker (only when it ran and failed).
    pub output: String,
}

/// A generated file to lay down: its natural path (e.g. `rust/demo.rs`) and its
/// already-formatted text. Only the file name is used inside the scaffold.
pub type File<'a> = (&'a Path, &'a str);

/// Whether a toolchain binary is runnable. We only care that it spawns (the
/// binary is on PATH), not its exit status: `--version` is not universal (`go`
/// spells it `go version` and errors on the flag), but a successful spawn still
/// proves the tool is present.
fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn file_name(path: &Path) -> &std::ffi::OsStr {
    path.file_name().unwrap_or(path.as_os_str())
}

fn write(dest: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Scaffold `dir` for `target` with `files`, run the checker, and report.
pub fn check(target: TargetKind, files: &[File], dir: &Path) -> Result<Outcome, String> {
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    match target {
        TargetKind::Rust => check_rust(files, dir),
        TargetKind::Go => check_go(files, dir),
        TargetKind::TypeScript => check_ts(files, dir),
    }
}

/// A library crate whose modules are the generated files. The generated Rust
/// references its serde companion as `crate::<module>_serde`, so each file
/// becomes a module of that exact name.
fn check_rust(files: &[File], dir: &Path) -> Result<Outcome, String> {
    let mut mods = String::new();
    for (path, text) in files {
        let name = file_name(path);
        write(&dir.join("src").join(name), text)?;
        let stem = Path::new(name).file_stem().unwrap().to_string_lossy();
        mods.push_str(&format!("pub mod {stem};\n"));
    }
    write(&dir.join("src/lib.rs"), &mods)?;
    write(
        &dir.join("Cargo.toml"),
        // Detached from any workspace; serde is what the generated models use.
        "[package]\nname = \"tono-preview\"\nversion = \"0.0.0\"\nedition = \"2021\"\npublish = false\n\n\
         [dependencies]\nserde = { version = \"1\", features = [\"derive\"] }\nserde_json = \"1\"\n\n\
         [workspace]\n",
    )?;
    if !have("cargo") {
        return Ok(skipped("cargo"));
    }
    run(
        Command::new("cargo")
            .args(["check", "--quiet"])
            .current_dir(dir),
        "cargo check",
    )
}

/// A single Go package directory. Generated Go uses only the standard library,
/// so a bare module with the emitted files type-checks offline.
fn check_go(files: &[File], dir: &Path) -> Result<Outcome, String> {
    for (path, text) in files {
        write(&dir.join(file_name(path)), text)?;
    }
    write(&dir.join("go.mod"), "module tonopreview\n\ngo 1.21\n")?;
    if !have("go") {
        return Ok(skipped("go"));
    }
    run(
        Command::new("go").args(["build", "./..."]).current_dir(dir),
        "go build",
    )
}

/// A tsconfig covering the emitted files, checked with `tsc --noEmit`.
/// Bundler resolution matches the generated code's extensionless relative
/// imports and, unlike the removed node10 mode, survives new tsc majors.
fn check_ts(files: &[File], dir: &Path) -> Result<Outcome, String> {
    for (path, text) in files {
        write(&dir.join(file_name(path)), text)?;
    }
    write(
        &dir.join("tsconfig.json"),
        "{\n  \"compilerOptions\": {\n    \"strict\": true,\n    \"target\": \"ES2020\",\n    \
         \"module\": \"ESNext\",\n    \"moduleResolution\": \"bundler\",\n    \"noEmit\": true,\n    \
         \"skipLibCheck\": true\n  },\n  \"include\": [\"*.ts\"]\n}\n",
    )?;
    if !have("tsc") {
        return Ok(skipped("tsc"));
    }
    run(Command::new("tsc").arg("--noEmit").current_dir(dir), "tsc")
}

fn skipped(tool: &str) -> Outcome {
    Outcome {
        ran: false,
        ok: false,
        tool: tool.into(),
        output: String::new(),
    }
}

/// Run a checker command, folding its exit status and captured output into an
/// [`Outcome`].
fn run(cmd: &mut Command, tool: &str) -> Result<Outcome, String> {
    let out = cmd.output().map_err(|e| format!("running {tool}: {e}"))?;
    let ok = out.status.success();
    let output = if ok {
        String::new()
    } else {
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        s
    };
    Ok(Outcome {
        ran: true,
        ok,
        tool: tool.into(),
        output,
    })
}

/// The default scaffold location when `--out` is not given.
pub fn default_dir() -> PathBuf {
    std::env::temp_dir().join(format!("tono-preview-{}", std::process::id()))
}

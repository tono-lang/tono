//! End-to-end checks of `tono index`: an `ext` block's libraries indexed
//! from stand-in packages under a temp project, the index written beside
//! the manifest with the key the editor recomputes. These need the built
//! frontend and, per language, `go`, `node` or `cargo`; each skips cleanly
//! when its tool is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

fn frontend() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TONO_FRONTEND") {
        let p = PathBuf::from(p);
        return p.exists().then_some(p);
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let p = root.join("_build/default/frontend/bin/tono_frontend.exe");
    p.exists().then_some(p)
}

fn tono() -> Option<Command> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tono"));
    cmd.env("TONO_FRONTEND", frontend()?);
    Some(cmd)
}

fn have(tool: &str) -> bool {
    Command::new(tool).arg("version").output().is_ok()
}

macro_rules! skip_without {
    ($tool:expr) => {
        if !have($tool) {
            eprintln!("skipping: {} is not installed", $tool);
            return;
        }
    };
}

macro_rules! skip_without_frontend {
    () => {
        match tono() {
            Some(c) => c,
            None => {
                eprintln!("skipping: frontend binary not built (set TONO_FRONTEND)");
                return;
            }
        }
    };
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-index-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // The command reports paths from the manifest's canonical directory
    // (`/private/var` on macOS, not the `/var` symlink).
    std::fs::canonicalize(&dir).unwrap()
}

fn write(path: &Path, contents: &str) -> PathBuf {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
    path.to_path_buf()
}

const GO_SOURCE: &str = "\
ext gearbox {
  go { #(example.test/gearbox) }

  struct dial {
    go { #(Dial[float64]) }

    op read(): float {
      go { call: #(Read)(#(ctx context.Context)) }
    }
  }

  op open(value: float): dial {
    go { call: #(Open[float64])(value) }
  }
}
";

/// Run `tono index <file> --json` and parse the report lines.
fn index_json(
    mut cmd: Command,
    file: &Path,
    extra: &[&str],
) -> (bool, Vec<serde_json::Value>, String) {
    cmd.arg("index").arg(file).arg("--json").args(extra);
    let out = cmd.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let lines = stdout
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}")))
        .collect();
    (
        out.status.success(),
        lines,
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn without_a_manifest_the_report_is_one_error_line() {
    let cmd = skip_without_frontend!();
    let dir = tmp("no-manifest");
    let file = write(&dir.join("svc.tono"), GO_SOURCE);
    let (ok, lines, _) = index_json(cmd, &file, &[]);
    assert!(!ok);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "error");
    assert!(
        lines[0]["message"]
            .as_str()
            .unwrap()
            .contains("no tono.toml above"),
        "{lines:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pair_without_a_pinned_version_is_skipped_with_the_reason() {
    let cmd = skip_without_frontend!();
    let dir = tmp("no-pin");
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.go]\nout = \"sdk/go\"\n",
    );
    std::fs::create_dir_all(dir.join("sdk/go")).unwrap();
    let file = write(&dir.join("svc.tono"), GO_SOURCE);
    let (ok, lines, stderr) = index_json(cmd, &file, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "skipped");
    assert_eq!(lines[0]["ext"], "gearbox");
    assert_eq!(lines[0]["lang"], "go");
    assert!(
        lines[0]["reason"]
            .as_str()
            .unwrap()
            .contains("no go version pinned in [ext.gearbox]"),
        "{lines:?}"
    );
    assert!(!dir.join(".tono").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pair_whose_root_is_missing_is_skipped_and_only_narrows_the_pairs() {
    let cmd = skip_without_frontend!();
    let dir = tmp("no-root");
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.go]\nout = \"sdk/go\"\n\n[ext.gearbox]\ngo = \"v0.0.0\"\n",
    );
    let file = write(&dir.join("svc.tono"), GO_SOURCE);
    let (ok, lines, stderr) = index_json(cmd, &file, &["--only", "gearbox=go"]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "skipped");
    assert!(
        lines[0]["reason"]
            .as_str()
            .unwrap()
            .contains("does not exist (run tono gen first)"),
        "{lines:?}"
    );
    let (ok, lines, _) = index_json(tono().unwrap(), &file, &["--only", "gearbox=rust"]);
    assert!(ok);
    assert!(lines.is_empty(), "{lines:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A Go module that requires the stand-in `gearbox` library by a `replace`
/// to a sibling directory, the way a consumer pins a private dependency.
fn go_root(dir: &Path) -> PathBuf {
    let lib = dir.join("gearbox-go");
    write(
        &lib.join("go.mod"),
        "module example.test/gearbox\n\ngo 1.21\n",
    );
    write(
        &lib.join("gearbox.go"),
        "package gearbox\n\nimport \"context\"\n\n\
         type Dial[T any] interface{ Read(ctx context.Context) (T, error) }\n\n\
         func Open[T any](value T) (Dial[T], error) { return nil, nil }\n",
    );
    let root = dir.join("sdk/go");
    write(
        &root.join("go.mod"),
        &format!(
            "module example.test/consumer\n\ngo 1.21\n\nrequire example.test/gearbox v0.0.0\n\nreplace example.test/gearbox => {}\n",
            lib.display()
        ),
    );
    root
}

#[test]
fn a_go_library_is_indexed_beside_the_manifest_with_its_key() {
    let cmd = skip_without_frontend!();
    skip_without!("go");
    let dir = tmp("go");
    go_root(&dir);
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.go]\nout = \"sdk/go\"\n\n[ext.gearbox]\ngo = \"v0.0.0\"\n",
    );
    let file = write(&dir.join("svc.tono"), GO_SOURCE);
    let (ok, lines, stderr) = index_json(cmd, &file, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "built", "{lines:?}");
    assert_eq!(lines[0]["symbols"], 2);
    let path = dir.join(".tono/index/gearbox.go.json");
    assert_eq!(
        lines[0]["path"].as_str().unwrap(),
        path.to_string_lossy().as_ref()
    );
    let index: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(index["tono_index_version"], 1);
    assert_eq!(index["key"]["ext"], "gearbox");
    assert_eq!(index["key"]["lang"], "go");
    assert_eq!(index["key"]["package"], "example.test/gearbox");
    assert_eq!(index["key"]["version"], "v0.0.0");
    assert_eq!(index["key"]["format"], 1);
    let lockfile = index["key"]["lockfile"]["path"].as_str().unwrap();
    assert!(lockfile.ends_with("sdk/go/go.sum"), "{lockfile}");
    assert_eq!(index["key"]["lockfile"]["digest"], "none");
    let names: Vec<&str> = index["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Dial", "Open"]);
    assert_eq!(index["symbols"][0]["members"][0]["name"], "Read");
    // A second run writes the same bytes: the index is a function of its key.
    let before = std::fs::read(&path).unwrap();
    let (ok, _, _) = index_json(tono().unwrap(), &file, &[]);
    assert!(ok);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The TypeScript compiler API this checkout can test with: the alias the
/// codegen tests install (`npm install` in backend/codegen-tests/typescript).
fn ts_api() -> Option<PathBuf> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let alias = repo.join("backend/codegen-tests/typescript/node_modules/typescript-api");
    alias.join("lib/typescript.js").is_file().then_some(alias)
}

const TS_SOURCE: &str = "\
ext gearbox {
  ts { #(@example/gearbox) }

  struct dial {
    ts { #(Dial) }

    op read(): float {
      ts { call: #(read)() }
    }
  }

  op open(value: float): dial {
    ts { call: #(new Dial)(value) }
  }
}
";

#[test]
fn a_typescript_library_is_indexed_through_the_compiler_api() {
    let mut cmd = skip_without_frontend!();
    if !Command::new("node").arg("--version").output().is_ok() {
        eprintln!("skipping: node is not installed");
        return;
    }
    let Some(api) = ts_api() else {
        eprintln!("skipping: no TypeScript compiler API (npm install in backend/codegen-tests/typescript)");
        return;
    };
    let dir = tmp("ts");
    let pkg = dir.join("sdk/ts/node_modules/@example/gearbox");
    write(
        &pkg.join("package.json"),
        "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"index.d.ts\"}",
    );
    write(
        &pkg.join("index.d.ts"),
        "export * from \"./more\";\nexport declare class Dial { constructor(value: number); read(): number; }\n",
    );
    write(
        &pkg.join("more.d.ts"),
        "export declare function calibrate(d: Dial): void;\n",
    );
    write(&dir.join("sdk/ts/package-lock.json"), "{}");
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.typescript]\nout = \"sdk/ts\"\n\n[ext.gearbox]\nts = \"0.0.0\"\n",
    );
    let file = write(&dir.join("svc.tono"), TS_SOURCE);
    cmd.env("TONO_TYPESCRIPT", &api);
    let (ok, lines, stderr) = index_json(cmd, &file, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "built", "{lines:?}");
    assert_eq!(lines[0]["symbols"], 2);
    let path = dir.join(".tono/index/gearbox.ts.json");
    let index: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(index["key"]["package"], "@example/gearbox");
    assert_eq!(index["key"]["version"], "0.0.0");
    let lockfile = index["key"]["lockfile"]["path"].as_str().unwrap();
    assert!(lockfile.ends_with("sdk/ts/package-lock.json"), "{lockfile}");
    // FNV-1a 64 of "{}", the lockfile's bytes.
    assert_eq!(index["key"]["lockfile"]["digest"], "08f44b07b5901a25");
    let names: Vec<&str> = index["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Dial", "calibrate"]);
    assert_eq!(index["symbols"][0]["kind"], "class");
    assert_eq!(index["symbols"][0]["members"][0]["name"], "read");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_text_form_prints_the_same_lines_on_stderr() {
    let mut cmd = skip_without_frontend!();
    let dir = tmp("text");
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.go]\nout = \"sdk/go\"\n",
    );
    let file = write(&dir.join("svc.tono"), GO_SOURCE);
    let out = cmd.arg("index").arg(&file).output().unwrap();
    assert!(out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("skipped: gearbox/go: no go version pinned"),
        "{stderr}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

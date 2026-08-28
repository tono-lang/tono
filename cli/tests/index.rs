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
    if Command::new("node").arg("--version").output().is_err() {
        eprintln!("skipping: node is not installed");
        return;
    }
    let Some(api) = ts_api() else {
        eprintln!("skipping: no TypeScript compiler API (npm install in backend/codegen-tests/typescript)");
        return;
    };
    let dir = tmp("ts");
    // The tree a user has: `tono init` scaffolds the manifest and the SDK's
    // own package.json (`"type": "module"`) under dist/typescript, and the
    // library is installed there. The helper runs under that package.json.
    let init = tono()
        .unwrap()
        .args(["init", "--target", "typescript", "--yes"])
        .current_dir(&dir)
        .output()
        .unwrap();
    assert!(
        init.status.success(),
        "{}",
        String::from_utf8_lossy(&init.stderr)
    );
    let sdk = dir.join("dist/typescript");
    assert!(std::fs::read_to_string(sdk.join("package.json"))
        .unwrap()
        .contains("\"type\": \"module\""));
    let pkg = sdk.join("node_modules/@example/gearbox");
    write(
        &pkg.join("package.json"),
        "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"index.d.ts\"}",
    );
    write(
        &pkg.join("index.d.ts"),
        "export * from \"./more\";\n\
         export { Gauge as Meter } from \"./extra\";\n\
         /** Build a dial.\n *\n * Second paragraph. */\n\
         export declare function build(name: string): Dial;\n\
         export declare function build(name: string, size: number): Dial;\n\
         export type Size = \"s\" | \"m\";\n\
         export declare const VERSION: string;\n\
         export declare const make: (name: string) => Dial;\n\
         export declare class Dial {\n  constructor(value: number);\n  static create(name: string): Dial;\n  private secret: number;\n  readonly name: string;\n  read(depth?: number): number;\n}\n\
         export interface Options { size?: Size; verbose: boolean }\n",
    );
    write(
        &pkg.join("more.d.ts"),
        "export declare function calibrate(d: Dial): void;\n\
         export declare enum Mode { Fast, Slow }\n\
         export declare namespace util { function pad(s: string): string; namespace deep { const x: number } }\n",
    );
    write(
        &pkg.join("extra.d.ts"),
        "export declare class Gauge { level(): number }\n",
    );
    write(&sdk.join("package-lock.json"), "{}");
    let manifest = std::fs::read_to_string(dir.join("tono.toml")).unwrap();
    write(
        &dir.join("tono.toml"),
        &format!("{manifest}\n[ext.gearbox]\nts = \"0.0.0\"\n"),
    );
    let file = write(&dir.join("svc.tono"), TS_SOURCE);
    cmd.env("TONO_TYPESCRIPT", &api);
    let (ok, lines, stderr) = index_json(cmd, &file, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "built", "{lines:?}");
    assert_eq!(lines[0]["symbols"], 10);
    let path = dir.join(".tono/index/gearbox.ts.json");
    let index: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(index["key"]["package"], "@example/gearbox");
    assert_eq!(index["key"]["version"], "0.0.0");
    let lockfile = index["key"]["lockfile"]["path"].as_str().unwrap();
    assert!(
        lockfile.ends_with("dist/typescript/package-lock.json"),
        "{lockfile}"
    );
    // FNV-1a 64 of "{}", the lockfile's bytes.
    assert_eq!(index["key"]["lockfile"]["digest"], "08f44b07b5901a25");
    let names: Vec<&str> = index["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec![
            "Dial",
            "Meter",
            "Mode",
            "Options",
            "Size",
            "VERSION",
            "build",
            "calibrate",
            "make",
            "util"
        ]
    );
    let by = |n: &str| {
        index["symbols"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["name"] == n)
            .unwrap()
            .clone()
    };
    // Overloads stay on one entry; the doc is the first paragraph.
    let build = by("build");
    assert_eq!(build["kind"], "function");
    assert_eq!(
        build["signatures"],
        serde_json::json!(["(name: string): Dial", "(name: string, size: number): Dial"])
    );
    assert_eq!(build["doc"], "Build a dial.");
    // A class: its constructor, statics first, private members out.
    let dial = by("Dial");
    assert_eq!(dial["kind"], "class");
    assert_eq!(dial["signatures"][0], "(value: number): Dial");
    let members: Vec<(String, String, bool)> = dial["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["name"].as_str().unwrap().into(),
                m["kind"].as_str().unwrap().into(),
                m["static"].as_bool().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        members,
        vec![
            ("create".to_string(), "method".to_string(), true),
            ("name".to_string(), "field".to_string(), false),
            ("read".to_string(), "method".to_string(), false),
        ]
    );
    // Re-exports followed: the glob and the renamed one.
    assert_eq!(by("calibrate")["kind"], "function");
    assert_eq!(by("Meter")["kind"], "class");
    assert_eq!(by("Meter")["members"][0]["name"], "level");
    assert_eq!(by("make")["kind"], "function");
    assert_eq!(by("VERSION")["kind"], "const");
    assert_eq!(by("VERSION")["signatures"][0], "string");
    assert_eq!(by("Size")["kind"], "type");
    assert_eq!(by("Size")["signatures"][0], "\"s\" | \"m\"");
    let mode = by("Mode");
    assert_eq!(mode["kind"], "enum");
    assert_eq!(mode["members"][0]["name"], "Fast");
    assert_eq!(mode["members"][1]["name"], "Slow");
    let util = by("util");
    assert_eq!(util["kind"], "namespace");
    assert_eq!(util["members"][0]["name"], "deep.x");
    assert_eq!(util["members"][0]["kind"], "const");
    assert_eq!(util["members"][1]["name"], "pad");
    assert_eq!(util["members"][1]["kind"], "function");
    assert_eq!(by("Options")["kind"], "interface");
    assert_eq!(by("Options")["members"][0]["signatures"][0], "Size");
    // As a user runs it: from the project directory, the file relative.
    let before = std::fs::read(&path).unwrap();
    let mut relative = tono().unwrap();
    relative
        .current_dir(&dir)
        .env("TONO_TYPESCRIPT", &api)
        .args(["index", "svc.tono", "--json"]);
    let out = relative.output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("\"kind\":\"built\""), "{stdout}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let _ = std::fs::remove_dir_all(&dir);
}

const RUST_SOURCE: &str = "\
ext gearbox {
  rust { #(gearbox) }

  struct dial {
    rust { #(Dial<f64>) }

    op read(): float {
      rust { call: #(read)() }
    }
  }

  op open(value: float): dial {
    rust { call: #(open)(value) }
  }
}
";

#[test]
fn a_rust_crate_is_indexed_from_its_source_through_cargo_metadata() {
    let cmd = skip_without_frontend!();
    if Command::new("cargo").arg("--version").output().is_err() {
        eprintln!("skipping: cargo is not installed");
        return;
    }
    let dir = tmp("rust");
    let lib = dir.join("gearbox-rs");
    write(
        &lib.join("Cargo.toml"),
        "[package]\nname = \"gearbox\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[workspace]\n",
    );
    write(
        &lib.join("src/lib.rs"),
        "mod dial;\npub use dial::Dial;\n/// Open a dial.\npub fn open<T>(value: T) -> Dial<T> { Dial { value } }\n",
    );
    write(
        &lib.join("src/dial.rs"),
        "pub struct Dial<T> { pub value: T }\nimpl<T: Copy> Dial<T> { pub fn read(&self) -> T { self.value } }\n",
    );
    let root = dir.join("sdk/rust");
    write(&root.join("src/lib.rs"), "");
    write(
        &root.join("Cargo.toml"),
        &format!(
            "[package]\nname = \"sdk\"\nversion = \"0.0.0\"\nedition = \"2021\"\n\n[dependencies]\ngearbox = {{ path = {:?} }}\n\n[workspace]\n",
            lib.display()
        ),
    );
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n\n[target.rust]\nout = \"sdk/rust\"\n\n[ext.gearbox]\nrust = \"0.0.0\"\n",
    );
    let file = write(&dir.join("svc.tono"), RUST_SOURCE);
    let (ok, lines, stderr) = index_json(cmd, &file, &[]);
    assert!(ok, "{stderr}");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert_eq!(lines[0]["kind"], "built", "{lines:?}");
    assert_eq!(lines[0]["symbols"], 2);
    assert!(
        lines[0]["note"]
            .as_str()
            .unwrap()
            .contains("what a macro produces is not indexed"),
        "{lines:?}"
    );
    let path = dir.join(".tono/index/gearbox.rust.json");
    let index: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let lockfile = index["key"]["lockfile"]["path"].as_str().unwrap();
    assert!(lockfile.ends_with("sdk/rust/Cargo.lock"), "{lockfile}");
    assert_ne!(index["key"]["lockfile"]["digest"], "none");
    let names: Vec<&str> = index["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["Dial", "open"]);
    assert_eq!(index["symbols"][0]["kind"], "struct");
    assert_eq!(index["symbols"][0]["members"][0]["name"], "read");
    assert_eq!(index["symbols"][0]["members"][1]["name"], "value");
    assert_eq!(index["symbols"][1]["doc"], "Open a dial.");
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

//! End-to-end checks of `tono check` past the frontend: an `ext` block's
//! bindings checked against a stand-in library with the target toolchain,
//! reported on the `.tono`. These need the built frontend and, per target,
//! `go` or `tsc`; each skips cleanly when its tool is absent.

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

fn have(tool: &str) -> bool {
    Command::new(tool).arg("--version").output().is_ok()
}

fn tono() -> Option<Command> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_tono"));
    cmd.env("TONO_FRONTEND", frontend()?);
    Some(cmd)
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
    let dir = std::env::temp_dir().join(format!("tono-check-it-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) -> PathBuf {
    std::fs::write(path, contents).unwrap();
    path.to_path_buf()
}

/// A Go module that requires the stand-in `gearbox` library by a `replace`
/// to a sibling directory, the way a consumer pins a private dependency.
fn go_root(dir: &Path) -> PathBuf {
    let lib = dir.join("gearbox-go");
    std::fs::create_dir_all(&lib).unwrap();
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
    let root = dir.join("consumer-go");
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root.join("go.mod"),
        &format!(
            "module example.test/consumer\n\ngo 1.21\n\nrequire example.test/gearbox v0.0.0\n\nreplace example.test/gearbox => {}\n",
            lib.display()
        ),
    );
    root
}

/// A tree whose `node_modules` holds the stand-in package's declarations.
fn ts_root(dir: &Path) -> PathBuf {
    let root = dir.join("consumer-ts");
    let pkg = root.join("node_modules/@example/gearbox");
    std::fs::create_dir_all(&pkg).unwrap();
    write(
        &pkg.join("package.json"),
        "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"index.d.ts\"}",
    );
    write(
        &pkg.join("index.d.ts"),
        "export interface Dial<T> { read(): T; }\nexport class Dial<T> { constructor(value: T); }\n",
    );
    root
}

const GO_OK: &str = "\
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

fn run(mut cmd: Command, file: &Path, roots: &[(&str, &Path)]) -> (bool, String) {
    cmd.arg("check").arg(file);
    for (lang, dir) in roots {
        cmd.arg("--lib-root")
            .arg(format!("{lang}={}", dir.display()));
    }
    let out = cmd.output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn go_bindings_that_match_the_library_pass() {
    let cmd = skip_without_frontend!();
    skip_without!("go");
    let dir = tmp("go-ok");
    let root = go_root(&dir);
    let file = write(&dir.join("svc.tono"), GO_OK);
    let (ok, err) = run(cmd, &file, &[("go", &root)]);
    assert!(ok, "{err}");
    assert!(
        err.contains("checked: go bindings of ext gearbox (go build)"),
        "{err}"
    );
    assert!(err.ends_with(&format!("ok: {}\n", file.display())), "{err}");
    assert!(
        !std::fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")),
        "the probe directory is cleaned up"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_go_binding_that_diverges_is_reported_at_its_tono_span() {
    let cmd = skip_without_frontend!();
    skip_without!("go");
    let dir = tmp("go-bad");
    let root = go_root(&dir);
    // Two arguments where the library's Open takes one.
    let src = GO_OK.replace("#(Open[float64])(value)", "#(Open[float64])(value, value)");
    let file = write(&dir.join("svc.tono"), &src);
    let (ok, err) = run(cmd, &file, &[("go", &root)]);
    assert!(!ok, "{err}");
    let line = err
        .lines()
        .find(|l| l.contains("FX0001"))
        .unwrap_or_else(|| panic!("no finding in:\n{err}"));
    assert!(
        line.starts_with(
            "13:16-45: error: FX0001: go binding of op open in ext gearbox: too many arguments"
        ),
        "{line}"
    );
    assert!(!err.contains("ok:"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ts_bindings_are_checked_against_the_declarations_file() {
    let cmd = skip_without_frontend!();
    skip_without!("tsc");
    let dir = tmp("ts");
    let root = ts_root(&dir);
    let src = "\
ext gearbox {
  ts { #(@example/gearbox) }

  struct dial {
    ts { #(Dial<number>) }

    @async(ts)
    op read(): float {
      ts { call: #(read)() }
    }
  }

  op open(value: float): dial {
    ts { call: #(new Dial)(value) }
  }
}
";
    let file = write(&dir.join("svc.tono"), src);
    let (ok, err) = run(cmd, &file, &[("ts", &root)]);
    // read() is synchronous in the library, the op declares it async here.
    assert!(!ok, "{err}");
    assert!(
        err.contains(
            "9:18-25: error: FX0001: ts binding of method dial.read in ext gearbox: error TS2322"
        ),
        "{err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_library_without_a_type_source_is_reported_not_failed() {
    let cmd = skip_without_frontend!();
    skip_without!("go");
    let dir = tmp("go-missing");
    let root = dir.join("consumer-go");
    std::fs::create_dir_all(&root).unwrap();
    write(
        &root.join("go.mod"),
        "module example.test/consumer\n\ngo 1.21\n",
    );
    let file = write(&dir.join("svc.tono"), GO_OK);
    let (ok, err) = run(cmd, &file, &[("go", &root)]);
    assert!(ok, "{err}");
    assert!(
        err.contains("not checked: go bindings of ext gearbox: no type source ("),
        "{err}"
    );
    assert!(!err.contains("FX0001"), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn without_a_root_every_language_is_reported_unchecked() {
    let cmd = skip_without_frontend!();
    let dir = tmp("no-root");
    let src = GO_OK.replace(
        "  go { #(example.test/gearbox) }\n",
        "  go { #(example.test/gearbox) }\n  rust { #(gearbox) }\n",
    );
    let file = write(&dir.join("svc.tono"), &src);
    let (ok, err) = run(cmd, &file, &[]);
    assert!(ok, "{err}");
    assert!(err.contains("not checked: rust bindings of ext gearbox: reading a crate's signatures needs rustdoc JSON"), "{err}");
    assert!(
        err.contains("not checked: go bindings of ext gearbox: no Go module"),
        "{err}"
    );
    assert!(err.ends_with(&format!("ok: {}\n", file.display())), "{err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_manifest_names_the_roots_and_a_missing_out_dir_is_reported() {
    let cmd = skip_without_frontend!();
    skip_without!("go");
    let dir = tmp("manifest");
    let root = go_root(&dir);
    write(
        &dir.join("tono.toml"),
        "[project]\nname = \"svc\"\n[target.go]\nout = \"consumer-go\"\n[target.typescript]\nout = \"dist/ts\"\n",
    );
    let file = write(&dir.join("svc.tono"), GO_OK);
    let (ok, err) = run(cmd, &file, &[]);
    assert!(ok, "{err}");
    assert!(
        err.contains("checked: go bindings of ext gearbox (go build)"),
        "{err}"
    );
    assert!(
        err.contains("not checked: ts bindings: the library root"),
        "{err}"
    );
    assert!(err.contains("does not exist"), "{err}");
    let _ = root;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_source_without_ext_blocks_needs_no_toolchain() {
    let cmd = skip_without_frontend!();
    let dir = tmp("plain");
    let file = write(
        &dir.join("plain.tono"),
        "struct point { x: i64\n y: i64 }\n",
    );
    let (ok, err) = run(cmd, &file, &[]);
    assert!(ok, "{err}");
    assert_eq!(err, format!("ok: {}\n", file.display()));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn usage_errors_are_reported() {
    let mut cmd = skip_without_frontend!();
    let out = cmd.args(["check"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("missing <file.tono>"));
    let mut cmd = tono().unwrap();
    let out = cmd
        .args(["check", "x.tono", "--lib-root", "go"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("<lang>=<dir>"));
}

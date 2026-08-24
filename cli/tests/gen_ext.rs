//! End-to-end checks for ext dependency versions: `tono gen` emits
//! the pinned `[ext.<name>]` version into each target's native manifest, and
//! refuses to generate for an enabled target with no pin.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A module whose `demo` declares one `ext` block with a Go and a TypeScript
/// module path. No `extern`/struct content: `ext_deps_for` only reads
/// `ExtLib.langs`, so this is the minimal fixture that exercises it.
const IR_WITH_EXT: &str = r#"{"tono_ir_version":31,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[],"ext_libs":[{"name":"companyconfig","langs":[{"lang":"go","path":"github.com/company/config"},{"lang":"ts","path":"@company/config"},{"lang":"rust","path":"company-config"}]}]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-gen-ext-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_manifest(dir: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("tono.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn gen_stdin_in(cwd: &Path, ir: &str) -> std::process::Output {
    let mut child = tono()
        .args(["gen", "--config", "tono.toml"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(ir.as_bytes());
    child.wait_with_output().unwrap()
}

#[test]
fn an_unpinned_ext_used_by_an_enabled_target_fails_generation() {
    let dir = tmpdir("unpinned");
    write_manifest(&dir, "[target.go]\nenabled = true\nout = \"go\"\n");
    let out = gen_stdin_in(&dir, IR_WITH_EXT);
    assert!(
        !out.status.success(),
        "generation must fail on an unpinned ext"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("companyconfig"), "{stderr}");
    assert!(stderr.contains("go"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pinned_ext_version_is_emitted_into_go_mod() {
    let dir = tmpdir("go-mod");
    std::fs::create_dir_all(dir.join("go")).unwrap();
    std::fs::write(
        dir.join("go/go.mod"),
        "module example.com/demo\n\ngo 1.21\n",
    )
    .unwrap();
    write_manifest(
        &dir,
        "[target.go]\nenabled = true\nout = \"go\"\n\n[ext.companyconfig]\ngo = \"v1.4.2\"\n",
    );
    let out = gen_stdin_in(&dir, IR_WITH_EXT);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let go_mod = std::fs::read_to_string(dir.join("go/go.mod")).unwrap();
    assert!(go_mod.contains("module example.com/demo"), "{go_mod}");
    assert!(go_mod.contains("go 1.21"), "{go_mod}");
    assert!(
        go_mod.contains("require github.com/company/config v1.4.2"),
        "{go_mod}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_pinned_ext_version_is_emitted_into_cargo_toml_and_package_json() {
    let dir = tmpdir("rust-ts");
    std::fs::create_dir_all(dir.join("rust")).unwrap();
    std::fs::create_dir_all(dir.join("ts")).unwrap();
    std::fs::write(
        dir.join("rust/Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("ts/package.json"),
        "{\n  \"name\": \"demo\",\n  \"version\": \"0.1.0\"\n}\n",
    )
    .unwrap();
    write_manifest(
        &dir,
        "[target.rust]\nenabled = true\nout = \"rust\"\n\n\
         [target.typescript]\nenabled = true\nout = \"ts\"\n\n\
         [ext.companyconfig]\nrust = \"1.2.0\"\nts = \"^3.1.0\"\ngo = \"v1.4.2\"\n",
    );
    let out = gen_stdin_in(&dir, IR_WITH_EXT);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cargo_toml = std::fs::read_to_string(dir.join("rust/Cargo.toml")).unwrap();
    assert!(cargo_toml.contains("serde = \"1\""), "{cargo_toml}");
    assert!(
        cargo_toml.contains("company-config = \"1.2.0\""),
        "{cargo_toml}"
    );

    let package_json = std::fs::read_to_string(dir.join("ts/package.json")).unwrap();
    assert!(
        package_json.contains("\"name\": \"demo\""),
        "{package_json}"
    );
    assert!(
        package_json.contains("\"@company/config\": \"^3.1.0\""),
        "{package_json}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn regenerating_with_the_same_pin_does_not_churn_go_mod() {
    let dir = tmpdir("go-mod-idempotent");
    std::fs::create_dir_all(dir.join("go")).unwrap();
    std::fs::write(
        dir.join("go/go.mod"),
        "module example.com/demo\n\ngo 1.21\n",
    )
    .unwrap();
    write_manifest(
        &dir,
        "[target.go]\nenabled = true\nout = \"go\"\n\n[ext.companyconfig]\ngo = \"v1.4.2\"\n",
    );
    let first = gen_stdin_in(&dir, IR_WITH_EXT);
    assert!(first.status.success());
    let after_first = std::fs::read_to_string(dir.join("go/go.mod")).unwrap();

    let second = gen_stdin_in(&dir, IR_WITH_EXT);
    assert!(second.status.success());
    let after_second = std::fs::read_to_string(dir.join("go/go.mod")).unwrap();
    assert_eq!(after_first, after_second);

    let _ = std::fs::remove_dir_all(&dir);
}

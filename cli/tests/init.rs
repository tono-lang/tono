//! End-to-end checks of `tono init`: manifest creation, idempotent updates,
//! and that `tono gen` works immediately afterward with no extra flags.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const IR: &str = r#"{"tono_ir_version":7,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-cli-init-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Run `tono init` in `dir` with `extra` flags, stdin closed so a missing
/// `--target` in a non-`--yes` run fails instead of blocking on a prompt.
fn init(dir: &Path, extra: &[&str]) -> (bool, String, String) {
    let out = tono()
        .current_dir(dir)
        .arg("init")
        .args(extra)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run `tono gen` in `dir` feeding `IR` on stdin, with no other flags (relies
/// on auto-discovery of the `tono.toml` written by `init`).
fn gen_via_stdin(dir: &Path) -> bool {
    let mut child = tono()
        .current_dir(dir)
        .arg("gen")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(IR.as_bytes())
        .unwrap();
    child.wait().unwrap().success()
}

#[test]
fn fresh_mode_writes_manifest_and_native_manifests() {
    let dir = tmpdir("fresh");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust,go,typescript"]);
    assert!(ok, "init failed: {stderr}");

    let manifest = std::fs::read_to_string(dir.join("tono.toml")).unwrap();
    assert!(manifest.contains("[target.rust]"));
    assert!(manifest.contains("[target.go]"));
    assert!(manifest.contains("[target.typescript]"));
    assert!(manifest.contains("baseline = \"git:last-tag\""));

    assert!(std::fs::read_to_string(dir.join("dist/rust/Cargo.toml"))
        .unwrap()
        .contains("[dependencies]"));
    let go_mod = std::fs::read_to_string(dir.join("dist/go/go.mod")).unwrap();
    assert!(go_mod.contains("go 1.21"));
    // The manifest's [target.go].package and go.mod's module directive must
    // agree: main.rs threads the manifest's `package` through as the Go
    // module path for cross-package imports (see `codegen_config_for`). Both
    // must also be an unambiguous placeholder, not a bare name that happens
    // to double as a valid (if wrong) module path.
    let slug = dir.file_name().unwrap().to_string_lossy().to_lowercase();
    let expected_module = format!("example.com/{slug}");
    assert!(
        manifest.contains(&format!("package = \"{expected_module}\"")),
        "{manifest}"
    );
    assert!(
        go_mod.contains(&format!("module {expected_module}")),
        "{go_mod}"
    );

    assert!(
        std::fs::read_to_string(dir.join("dist/typescript/package.json"))
            .unwrap()
            .contains("\"type\": \"module\"")
    );

    // A starter .gitignore, with the dist entry left as the user's call.
    let gitignore = std::fs::read_to_string(dir.join(".gitignore")).unwrap();
    assert!(gitignore.contains("node_modules/"), "{gitignore}");
    assert!(gitignore.contains("# /dist"), "{gitignore}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_existing_gitignore_is_never_touched() {
    let dir = tmpdir("gitignore-kept");
    std::fs::write(dir.join(".gitignore"), "mine\n").unwrap();
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "init failed: {stderr}");
    assert_eq!(
        std::fs::read_to_string(dir.join(".gitignore")).unwrap(),
        "mine\n"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gen_works_immediately_after_init_with_no_extra_flags() {
    let dir = tmpdir("gen-after-init");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust,go,typescript"]);
    assert!(ok, "init failed: {stderr}");

    assert!(gen_via_stdin(&dir), "gen should auto-discover tono.toml");
    for (sub, ext) in [("rust", "rs"), ("go", "go"), ("typescript", "ts")] {
        let path = dir
            .join("dist")
            .join(sub)
            .join("demo")
            .join(format!("types.{ext}"));
        assert!(path.is_file(), "{} was not generated", path.display());
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_mode_re_running_an_existing_target_is_a_no_op() {
    let dir = tmpdir("update-idempotent");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "init failed: {stderr}");
    let before = std::fs::read_to_string(dir.join("tono.toml")).unwrap();

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "second init failed: {stderr}");
    let after = std::fs::read_to_string(dir.join("tono.toml")).unwrap();

    assert_eq!(
        before, after,
        "re-running init for an existing target must not change the file"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_mode_appends_a_new_target_without_disturbing_existing_content() {
    let dir = tmpdir("update-append");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "init failed: {stderr}");
    let before = std::fs::read_to_string(dir.join("tono.toml")).unwrap();

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "python"]);
    assert!(ok, "update init failed: {stderr}");
    let after = std::fs::read_to_string(dir.join("tono.toml")).unwrap();

    assert!(
        after.starts_with(&before),
        "existing content must be an untouched prefix of the updated file"
    );
    assert!(after.contains("[target.python]"));
    assert!(after.contains("enabled = false"));
    // No native manifest for a placeholder target.
    assert!(!dir.join("python").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Adopting `init` on a project whose manifest was written by hand: the
/// targets are already declared, so nothing is appended, but the build setup
/// they are missing still gets created, at the `out` and under the `package`
/// that manifest configures.
#[test]
fn update_mode_scaffolds_a_declared_target_that_has_no_native_manifest() {
    let dir = tmpdir("update-existing-manifest");
    std::fs::write(
        dir.join("tono.toml"),
        "[project]\nname = \"acme\"\n\n\
         [target.rust]\nenabled = true\npackage = \"acme-sdk\"\nout = \"sdk/rust\"\n\n\
         [target.go]\nenabled = true\npackage = \"github.com/acme/pay\"\nout = \"sdk/go\"\n",
    )
    .unwrap();
    let before = std::fs::read_to_string(dir.join("tono.toml")).unwrap();

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust,go"]);
    assert!(ok, "init failed: {stderr}");
    // Status goes to stderr; stdout stays free of it.
    assert!(stderr.contains("already enabled"), "{stderr}");

    // The manifest itself is untouched: everything was already declared.
    assert_eq!(
        before,
        std::fs::read_to_string(dir.join("tono.toml")).unwrap()
    );

    let cargo = std::fs::read_to_string(dir.join("sdk/rust/Cargo.toml")).unwrap();
    assert!(cargo.contains("name = \"acme-sdk\""), "{cargo}");
    let go_mod = std::fs::read_to_string(dir.join("sdk/go/go.mod")).unwrap();
    // Verbatim from the manifest: a complete module path is never re-prefixed.
    assert!(go_mod.contains("module github.com/acme/pay"), "{go_mod}");

    let _ = std::fs::remove_dir_all(&dir);
}

/// A target that is declared but switched off is reported as such (with the
/// line to change) and left alone, rather than silently doing nothing.
#[test]
fn update_mode_explains_a_declared_but_disabled_target() {
    let dir = tmpdir("update-disabled");
    std::fs::write(
        dir.join("tono.toml"),
        "[project]\nname = \"acme\"\n\n[target.go]\nenabled = false\n",
    )
    .unwrap();

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "go"]);
    assert!(ok, "init failed: {stderr}");
    assert!(stderr.contains("disabled"), "{stderr}");
    assert!(stderr.contains("enabled = true"), "{stderr}");
    // Disabled by intent: no build setup is scaffolded for it.
    assert!(!dir.join("dist/go").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

/// `--root` writes `[project].root`, which update mode never rewrites, so a
/// run that cannot honour the flag says so rather than appearing to take it.
#[test]
fn root_is_reported_as_ignored_when_the_manifest_already_exists() {
    let dir = tmpdir("root-ignored");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "init failed: {stderr}");

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "go", "--root", "src"]);
    assert!(ok, "update init failed: {stderr}");
    assert!(stderr.contains("--root is ignored"), "{stderr}");
    // The flag really is inert: [project] keeps whatever it had.
    let manifest = std::fs::read_to_string(dir.join("tono.toml")).unwrap();
    assert!(!manifest.contains("root ="), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unknown_target_is_a_hard_error() {
    let dir = tmpdir("unknown-target");
    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "cobol"]);
    assert!(!ok);
    assert!(stderr.contains("unknown target 'cobol'"), "{stderr}");
    assert!(!dir.join("tono.toml").exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn non_interactive_without_a_target_errors_instead_of_hanging() {
    let dir = tmpdir("no-target");
    let (ok, _, stderr) = init(&dir, &["--yes"]);
    assert!(!ok);
    assert!(stderr.contains("no targets specified"), "{stderr}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn root_is_detected_from_an_existing_tono_file() {
    let dir = tmpdir("root-detect");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("src/demo.tono"), "").unwrap();

    let (ok, _, stderr) = init(&dir, &["--yes", "--target", "rust"]);
    assert!(ok, "init failed: {stderr}");

    let manifest = std::fs::read_to_string(dir.join("tono.toml")).unwrap();
    assert!(manifest.contains("root = \"src\""), "{manifest}");

    let _ = std::fs::remove_dir_all(&dir);
}

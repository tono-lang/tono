//! End-to-end checks of the `tono` binary: IR JSON in, SDK files out.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const IR: &str = r#"{"tono_ir_version":17,"modules":[{"name":"demo","shapes":[{"id":"demo#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// A contract extension with no conformance reference: the generator must refuse
/// to emit (AC-4). Everything else is well-formed.
const IR_UNCONFORMANT_CONTRACT: &str = r#"{"tono_ir_version":17,"modules":[{"name":"demo","shapes":[],"operations":[],"extensions":[{"name":"sign","kind":"contract","bindings":{"ts":"ext/ts/s.ts#sign"},"signature":{"input":{"prim":"string"},"output":{"prim":"string"}}}]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn tmpdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-cli-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Run `tono gen` feeding `IR` on stdin; returns whether it succeeded.
fn gen_via_stdin(out: &Path, target: &str) -> bool {
    let mut child = tono()
        .args(["gen", "--target", target, "--out", out.to_str().unwrap()])
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
fn version_prints_the_version() {
    let out = tono().arg("version").output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("tono "));
}

#[test]
fn gen_writes_a_file_per_target_from_stdin() {
    let out = tmpdir("stdin");
    assert!(gen_via_stdin(&out, "rust,go,typescript"));
    for (sub, ext) in [("rust", "rs"), ("go", "go"), ("typescript", "ts")] {
        let path = out.join(sub).join("demo").join(format!("types.{ext}"));
        let text = std::fs::read_to_string(&path).expect("generated file exists");
        assert!(text.contains("DO NOT EDIT"), "{sub} carries the banner");
    }
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_reads_the_ir_path_argument() {
    let out = tmpdir("path");
    std::fs::create_dir_all(&out).unwrap();
    let ir_path = out.join("ir.json");
    std::fs::write(&ir_path, IR).unwrap();
    let status = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            ir_path.to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success());
    assert!(out.join("rust").join("demo").join("types.rs").exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_requires_a_target() {
    let status = tono()
        .args(["gen", "--out", "/tmp/unused"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_rejects_an_unknown_target() {
    let status = tono()
        .args(["gen", "--target", "java", "--out", "/tmp/unused"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_rejects_invalid_ir() {
    let out = tmpdir("bad");
    let mut child = tono()
        .args(["gen", "--target", "rust", "--out", out.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(b"not json").unwrap();
    assert!(!child.wait().unwrap().success());
}

#[test]
fn gen_rejects_a_contract_without_conformance() {
    // The strong gate: a kind=contract extension with no conformance reference
    // must refuse to emit (build error), so the CLI exits non-zero.
    let out = tmpdir("gate");
    let mut child = tono()
        .args([
            "gen",
            "--target",
            "typescript",
            "--out",
            out.to_str().unwrap(),
        ])
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(IR_UNCONFORMANT_CONTRACT.as_bytes())
        .unwrap();
    assert!(!child.wait().unwrap().success());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_flag_without_a_value_fails() {
    let status = tono()
        .args(["gen", "--target"])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn gen_missing_ir_file_fails() {
    let out = tmpdir("missing");
    let status = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            "/no/such/ir.json",
        ])
        .stdin(Stdio::null())
        .status()
        .unwrap();
    assert!(!status.success());
}

#[test]
fn unknown_command_fails() {
    let status = tono().arg("wat").stdin(Stdio::null()).status().unwrap();
    assert!(!status.success());
}

/// A single dotted module, so the sub-package mapping and the config hooks have
/// something to place under a directory.
const DOTTED_IR: &str = r#"{"tono_ir_version":17,"modules":[{"name":"payments.common","shapes":[{"id":"payments.common#Money","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// Run `tono gen` with the given extra args, feeding `DOTTED_IR` on stdin.
fn gen_dotted(out: &Path, extra: &[&str]) {
    let mut args = vec!["gen", "--target", "rust", "--out", out.to_str().unwrap()];
    args.extend_from_slice(extra);
    let mut child = tono().args(&args).stdin(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(DOTTED_IR.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
}

#[test]
fn gen_maps_a_dotted_module_to_a_sub_package_path() {
    let out = tmpdir("subpkg");
    gen_dotted(&out, &[]);
    assert!(out
        .join("rust")
        .join("payments")
        .join("common")
        .join("types.rs")
        .exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_flatten_writes_a_flat_package_file() {
    let out = tmpdir("flatten");
    gen_dotted(&out, &["--flatten"]);
    assert!(out
        .join("rust")
        .join("payments_common")
        .join("types.rs")
        .exists());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn gen_module_remap_rewrites_the_prefix() {
    let out = tmpdir("remap");
    gen_dotted(&out, &["--module-remap", "payments=billing"]);
    assert!(out
        .join("rust")
        .join("billing")
        .join("common")
        .join("types.rs")
        .exists());
    let _ = std::fs::remove_dir_all(&out);
}

/// Two modules where one references the other across the boundary.
const TWO_MODULE_IR: &str = r#"{"tono_ir_version":17,"modules":[{"name":"payments.common","shapes":[{"id":"payments.common#Money","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]},{"name":"payments.charge","shapes":[{"id":"payments.charge#Charge","kind":"structure","params":[],"members":[{"name":"total","required":true,"target":{"ref":"payments.common#Money","args":[]},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// Run `tono gen --target go` with the given extra args, feeding `TWO_MODULE_IR`.
fn gen_two_module_go(out: &Path, extra: &[&str]) -> bool {
    let mut args = vec!["gen", "--target", "go", "--out", out.to_str().unwrap()];
    args.extend_from_slice(extra);
    let mut child = tono().args(&args).stdin(Stdio::piped()).spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(TWO_MODULE_IR.as_bytes())
        .unwrap();
    child.wait().unwrap().success()
}

#[test]
fn gen_multi_module_go_needs_a_module_path() {
    // Without --go-module the Go output could not compile, so it is rejected.
    let out = tmpdir("go-no-module");
    assert!(!gen_two_module_go(&out, &[]));
    // With the module path it succeeds.
    let ok = tmpdir("go-with-module");
    assert!(gen_two_module_go(&ok, &["--go-module", "example.com/sdk"]));
    let _ = std::fs::remove_dir_all(&out);
    let _ = std::fs::remove_dir_all(&ok);
}

// --- manifest mode ----------------------------------------------------------

/// A module with a multi-word field, so a casing override is observable.
const CASING_IR: &str = r#"{"tono_ir_version":17,"modules":[{"name":"demo","shapes":[{"id":"demo#Event","kind":"structure","params":[],"members":[{"name":"created_at","required":true,"target":{"prim":"string"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// Run `tono gen <extra>` from `cwd`, feeding `ir` on stdin; returns the output.
/// The write ignores a broken pipe: a manifest error makes the CLI exit before it
/// reads stdin, and the assertion under test is the exit status, not the write.
fn gen_stdin_in(cwd: &Path, extra: &[&str], ir: &str) -> std::process::Output {
    let mut child = tono()
        .arg("gen")
        .args(extra)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(ir.as_bytes());
    child.wait_with_output().unwrap()
}

/// Write `body` to `<dir>/tono.toml` and return its path.
fn write_manifest(dir: &Path, body: &str) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join("tono.toml");
    std::fs::write(&path, body).unwrap();
    path
}

fn ok_or_stderr(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn manifest_drives_enabled_targets_and_out_dirs() {
    let base = tmpdir("m-config");
    let manifest = write_manifest(
        &base,
        "[target.rust]\nout = \"gen/rust\"\n\n[target.typescript]\nout = \"gen/ts\"\n\n[target.python]\nenabled = false\n",
    );
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], IR);
    ok_or_stderr(&out);
    // Each enabled target lands under its own configured out (no shared root, no
    // `<target>/` prefix); the disabled python target emits nothing.
    assert!(base.join("gen/rust/demo/types.rs").is_file());
    assert!(base.join("gen/ts/demo/types.ts").is_file());
    assert!(!base.join("gen/python").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_split_repo_does_not_affect_generation() {
    let base = tmpdir("m-split-repo");
    // split_repo only feeds `tono split`; generation reads past it untouched.
    let manifest = write_manifest(
        &base,
        "[target.rust]\nout = \"gen/rust\"\nsplit_repo = \"acme/sdk-rust\"\n",
    );
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], IR);
    ok_or_stderr(&out);
    assert!(base.join("gen/rust/demo/types.rs").is_file());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_is_auto_discovered_from_a_subdirectory() {
    let base = tmpdir("m-discover");
    write_manifest(&base, "[target.go]\nout = \"out/go\"\n");
    let nested = base.join("sub");
    std::fs::create_dir_all(&nested).unwrap();
    let out = gen_stdin_in(&nested, &[], IR);
    ok_or_stderr(&out);
    // out resolves against the manifest's directory, not the working directory.
    assert!(base.join("out/go/demo/types.go").is_file());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_module_mapping_flat_flattens_the_layout() {
    let base = tmpdir("m-flat");
    let manifest = write_manifest(
        &base,
        "[target.rust]\nout = \"r\"\nmodule_mapping = \"flat\"\n",
    );
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], DOTTED_IR);
    ok_or_stderr(&out);
    // flat collapses payments.common into one segment; nested would be
    // r/payments/common/types.rs.
    assert!(base.join("r/payments_common/types.rs").is_file());
    assert!(!base.join("r/payments/common/types.rs").exists());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_module_remap_rewrites_the_prefix() {
    let base = tmpdir("m-remap");
    let manifest = write_manifest(
        &base,
        "[target.rust]\nout = \"r\"\n\n[target.rust.module_remap]\n\"payments\" = \"billing\"\n",
    );
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], DOTTED_IR);
    ok_or_stderr(&out);
    assert!(base.join("r/billing/common/types.rs").is_file());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_go_package_supplies_the_go_module_path() {
    // A multi-module Go SDK needs a module path; the manifest's package provides it.
    let ok = tmpdir("m-go-ok");
    let manifest = write_manifest(
        &ok,
        "[target.go]\nout = \"g\"\npackage = \"example.com/sdk\"\n",
    );
    let out = gen_stdin_in(
        &ok,
        &["--config", manifest.to_str().unwrap()],
        TWO_MODULE_IR,
    );
    ok_or_stderr(&out);
    assert!(ok.join("g/payments/common/types.go").is_file());

    // Without a package the Go layout check rejects the multi-module output.
    let bad = tmpdir("m-go-bad");
    let manifest = write_manifest(&bad, "[target.go]\nout = \"g\"\n");
    let out = gen_stdin_in(
        &bad,
        &["--config", manifest.to_str().unwrap()],
        TWO_MODULE_IR,
    );
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&ok);
    let _ = std::fs::remove_dir_all(&bad);
}

#[test]
fn manifest_casing_override_changes_the_emitted_field() {
    let base = tmpdir("m-casing");
    let manifest = write_manifest(
        &base,
        "[target.typescript]\nout = \"t\"\n\n[target.typescript.casing]\nfields = \"snake\"\n",
    );
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], CASING_IR);
    ok_or_stderr(&out);
    let text = std::fs::read_to_string(base.join("t/demo/types.ts")).unwrap();
    // TypeScript fields are camelCase by default; the override renders them snake.
    assert!(text.contains("created_at"), "{text}");
    assert!(!text.contains("createdAt"), "{text}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_enabling_an_unsupported_target_fails() {
    let base = tmpdir("m-unsupported");
    let manifest = write_manifest(&base, "[target.python]\nenabled = true\n");
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], IR);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("python"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_with_malformed_toml_fails() {
    let base = tmpdir("m-malformed");
    let manifest = write_manifest(&base, "not = = valid");
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], IR);
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn manifest_with_no_enabled_targets_fails() {
    let base = tmpdir("m-empty");
    let manifest = write_manifest(&base, "[target.rust]\nenabled = false\n");
    let out = gen_stdin_in(&base, &["--config", manifest.to_str().unwrap()], IR);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("no enabled targets"));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn gen_without_flags_or_manifest_fails() {
    let base = tmpdir("m-none");
    std::fs::create_dir_all(&base).unwrap();
    // No tono.toml up the tree and no flags: nothing to generate from.
    let out = gen_stdin_in(&base, &[], IR);
    assert!(!out.status.success());
    let _ = std::fs::remove_dir_all(&base);
}

/// `--clean` clears what a previous run generated and this one no longer does
/// (a module that was renamed or removed), including the directory it emptied.
#[test]
fn clean_removes_generated_files_this_run_did_not_produce() {
    let out = tmpdir("clean-stale");
    assert!(gen_via_stdin(&out, "rust"));
    // Output from an earlier shape of the project: same banner, no longer emitted.
    let stale_dir = out.join("rust").join("gone");
    std::fs::create_dir_all(&stale_dir).unwrap();
    std::fs::write(
        stale_dir.join("types.rs"),
        "// Code generated by tono. DO NOT EDIT.\n\npub struct Gone;\n",
    )
    .unwrap();

    let ok = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            "--clean",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|mut c| {
            c.stdin.take().unwrap().write_all(IR.as_bytes()).unwrap();
            c.wait().unwrap().success()
        })
        .unwrap();
    assert!(ok);

    assert!(!stale_dir.exists(), "the emptied directory goes too");
    assert!(
        out.join("rust/demo/types.rs").is_file(),
        "current output survives"
    );
    let _ = std::fs::remove_dir_all(&out);
}

/// The output directory belongs to the user: a sweep only removes files this
/// generator wrote, never a build manifest, a hand-written source, or a note.
#[test]
fn clean_never_removes_files_tono_did_not_generate() {
    let out = tmpdir("clean-keeps-handwritten");
    assert!(gen_via_stdin(&out, "rust"));
    let target = out.join("rust");
    std::fs::write(target.join("Cargo.toml"), "[package]\nname = \"mine\"\n").unwrap();
    std::fs::write(target.join("NOTES.md"), "mine\n").unwrap();
    std::fs::create_dir_all(target.join("hand")).unwrap();
    std::fs::write(target.join("hand/mine.rs"), "pub fn mine() {}\n").unwrap();

    let ok = tono()
        .args([
            "gen",
            "--target",
            "rust",
            "--out",
            out.to_str().unwrap(),
            "--clean",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map(|mut c| {
            c.stdin.take().unwrap().write_all(IR.as_bytes()).unwrap();
            c.wait().unwrap().success()
        })
        .unwrap();
    assert!(ok);

    for kept in ["Cargo.toml", "NOTES.md", "hand/mine.rs"] {
        assert!(target.join(kept).is_file(), "{kept} must survive a sweep");
    }
    let _ = std::fs::remove_dir_all(&out);
}

/// Without the flag nothing is swept: an unrelated generated file stays put.
#[test]
fn gen_leaves_stale_output_alone_without_clean() {
    let out = tmpdir("no-clean");
    assert!(gen_via_stdin(&out, "rust"));
    let stale = out.join("rust").join("stale.rs");
    std::fs::write(
        &stale,
        "// Code generated by tono. DO NOT EDIT.\n\npub struct S;\n",
    )
    .unwrap();
    assert!(gen_via_stdin(&out, "rust"));
    assert!(stale.is_file(), "no sweep without --clean");
    let _ = std::fs::remove_dir_all(&out);
}

/// Run `tono gen --config <manifest>` in `dir`, IR on stdin.
fn gen_with_manifest(dir: &Path) -> (bool, String) {
    let mut child = tono()
        .current_dir(dir)
        .args(["gen", "--config", "tono.toml"])
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
    let out = child.wait_with_output().unwrap();
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Generation owns the `exports` map, not the whole package manifest: the name,
/// version, `type` and dependencies a project keeps there survive a run.
#[test]
fn generated_package_json_merges_into_an_existing_manifest() {
    let dir = tmpdir("pkg-merge");
    std::fs::create_dir_all(dir.join("ts")).unwrap();
    std::fs::write(
        dir.join("tono.toml"),
        "[target.typescript]\nenabled = true\nout = \"ts\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("ts/package.json"),
        "{\n  \"name\": \"mine\",\n  \"version\": \"2.0.0\",\n  \"type\": \"module\",\n  \"dependencies\": { \"left-pad\": \"^1\" }\n}\n",
    )
    .unwrap();

    let (ok, stderr) = gen_with_manifest(&dir);
    assert!(ok, "gen failed: {stderr}");

    let text = std::fs::read_to_string(dir.join("ts/package.json")).unwrap();
    for kept in ["\"mine\"", "\"2.0.0\"", "\"module\"", "left-pad"] {
        assert!(text.contains(kept), "{kept} was lost:\n{text}");
    }
    assert!(text.contains("\"exports\""), "{text}");

    // A second run with the same input changes nothing, so the manifest does
    // not churn in review.
    let (ok, stderr) = gen_with_manifest(&dir);
    assert!(ok, "second gen failed: {stderr}");
    assert_eq!(
        text,
        std::fs::read_to_string(dir.join("ts/package.json")).unwrap()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// A manifest that cannot be parsed is left exactly as it is: overwriting it
/// would destroy whatever it holds, so the run stops instead.
#[test]
fn an_unparsable_package_json_stops_generation_untouched() {
    let dir = tmpdir("pkg-malformed");
    std::fs::create_dir_all(dir.join("ts")).unwrap();
    std::fs::write(
        dir.join("tono.toml"),
        "[target.typescript]\nenabled = true\nout = \"ts\"\n",
    )
    .unwrap();
    const BROKEN: &str = "{ not json\n";
    std::fs::write(dir.join("ts/package.json"), BROKEN).unwrap();

    let (ok, stderr) = gen_with_manifest(&dir);
    assert!(
        !ok,
        "generation must not succeed over an unreadable manifest"
    );
    assert!(stderr.contains("package.json"), "{stderr}");
    assert_eq!(
        BROKEN,
        std::fs::read_to_string(dir.join("ts/package.json")).unwrap()
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// An entry whose module declares tests inline in the IR: one hermetic test
/// stubbing the op's http dependency and one live test with no stub.
const IR_WITH_TESTS: &str = r#"{"tono_ir_version":17,"modules":[{"name":"demo","shapes":[{"id":"demo#client","kind":"entry","fields":[{"name":"endpoint","target":{"prim":"string"},"sources":["with",{"default":"https://example.com"}]}],"operations":[{"id":"demo#client.ping","kind":"operation","input":null,"output":{"prim":"string"},"errors":[],"traits":[],"wire":{"method":"GET","uri":{"template":[{"lit":"/ping"}]},"response_bindings":{},"success":[200],"endpoint":{"field":["endpoint"]}}}]}],"operations":[],"tests":[{"name":"answers","constructions":[{"binding":"c","entry":"client","values":{}}],"stubs":[{"binding":"s","client":"c","op":"ping","dep":"http","answers":[{"status":200,"headers":{},"body":"\"pong\""}]}],"calls":[{"binding":"got","client":"c","op":"ping"}],"expects":[{"subject":"got","pattern":{"eq":"pong"}}]},{"name":"answers for real","constructions":[{"binding":"c","entry":"client","values":{}}],"stubs":[],"calls":[{"binding":"got","client":"c","op":"ping"}],"expects":[{"subject":"got","pattern":{"eq":"pong"}}]}]}]}"#;

#[test]
fn gen_emits_native_tests_from_declared_tests() {
    let dir = tmpdir("declared-tests");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("ir.json"), IR_WITH_TESTS).unwrap();
    let out = dir.join("sdk");
    let output = tono()
        .args([
            "gen",
            "--target",
            "go",
            "--go-module",
            "example.com/sdk",
            "--out",
            out.to_str().unwrap(),
            dir.join("ir.json").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // The hermetic test lands in the package's own _test.go, the stubless one
    // in the live file behind the build tag.
    let hermetic =
        std::fs::read_to_string(out.join("go").join("demo").join("client_test.go")).unwrap();
    assert!(hermetic.contains("DO NOT EDIT"));
    assert!(hermetic.contains("func TestPingAnswers(t *testing.T)"));
    let live =
        std::fs::read_to_string(out.join("go").join("demo").join("client_live_test.go")).unwrap();
    assert!(live.contains("//go:build live"));
    assert!(live.contains("func TestPingAnswersForRealLive(t *testing.T)"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gen_refuses_an_invalid_declared_test() {
    // A test beyond the generated subset is a loud refusal naming the test,
    // never a silently skipped case.
    let dir = tmpdir("declared-tests-invalid");
    std::fs::create_dir_all(&dir).unwrap();
    let broken = IR_WITH_TESTS.replace(
        r#""calls":[{"binding":"got","client":"c","op":"ping"}],"expects":[{"subject":"got","pattern":{"eq":"pong"}}]},{"name":"answers for real""#,
        r#""calls":[{"binding":"got","client":"c","op":"ping"},{"binding":"again","client":"c","op":"ping"}],"expects":[]},{"name":"answers for real""#,
    );
    assert_ne!(broken, IR_WITH_TESTS);
    std::fs::write(dir.join("ir.json"), &broken).unwrap();
    let output = tono()
        .args([
            "gen",
            "--target",
            "go",
            "--go-module",
            "example.com/sdk",
            "--out",
            dir.join("sdk").to_str().unwrap(),
            dir.join("ir.json").to_str().unwrap(),
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("multi-call flows are not generated yet"),
        "{stderr}"
    );
    assert!(stderr.contains("answers"), "{stderr}");
    let _ = std::fs::remove_dir_all(&dir);
}

// Every IR fixture literal in this file embeds a bare version number; a
// stale one fails with a decode error far from this assertion. Catch it
// here instead: the same bump every past IR version change forgot.
#[test]
fn ir_fixtures_use_the_current_ir_version() {
    let expected = format!("\"tono_ir_version\":{}", tono_backend::ir::TONO_IR_VERSION);
    for (name, fixture) in [
        ("IR", IR),
        ("IR_UNCONFORMANT_CONTRACT", IR_UNCONFORMANT_CONTRACT),
        ("DOTTED_IR", DOTTED_IR),
        ("TWO_MODULE_IR", TWO_MODULE_IR),
        ("CASING_IR", CASING_IR),
        ("IR_WITH_TESTS", IR_WITH_TESTS),
    ] {
        assert!(
            fixture.contains(&expected),
            "stale tono_ir_version in {name}: {fixture}"
        );
    }
}

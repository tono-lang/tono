//! End-to-end checks of `tono split`: mirroring a target's generated SDK into
//! its own repository, in both snapshot (default) and subtree modes.
//!
//! Each test builds a throwaway monorepo and bare local repositories as
//! mirrors, then runs the command and inspects the mirrors with plain git.
//! Local paths stand in for the GitHub remotes; the push plumbing is
//! identical. The build feeding the split comes from a committed IR file
//! (the same reading contract `tono gen` has), so the suite runs without the
//! frontend binary; the one source-compiling test skips cleanly when the
//! frontend is absent. Mirror branches are named explicitly everywhere so the
//! suite is independent of the environment's init.defaultBranch.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The IR the monorepo commits: one module `spec` with a single structure.
const IR_V1: &str = r#"{"tono_ir_version":32,"modules":[{"name":"spec","shapes":[{"id":"spec#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

/// The evolved IR: the same structure grows a `currency` member.
const IR_V2: &str = r#"{"tono_ir_version":32,"modules":[{"name":"spec","shapes":[{"id":"spec#Charge","kind":"structure","params":[],"members":[{"name":"amount","required":true,"target":{"prim":"i64"},"constraints":[],"traits":[]},{"name":"currency","required":true,"target":{"prim":"string"},"constraints":[],"traits":[]}],"operations":[]}],"operations":[]}]}"#;

fn tono() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tono"))
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed");
}

/// Run git in `dir` and return its trimmed stdout.
fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A fresh scratch area holding the monorepo and its mirrors.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tono-split-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Init a git repo at `dir` with a committing identity configured.
fn init_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "t@example.com"]);
    git(dir, &["config", "user.name", "t"]);
}

/// Create a bare repository to act as a mirror remote. Its HEAD names `main`
/// (where every test pushes), so cloning it checks `main` out regardless of
/// the environment's init.defaultBranch.
fn bare_mirror(base: &Path, name: &str) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "--bare", "-b", "main"]);
    dir
}

fn write(dir: &Path, rel: &str, text: &str) {
    let path = dir.join(rel);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn commit_all(dir: &Path, msg: &str) {
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", msg]);
}

/// Build the standard monorepo: a manifest and the committed IR.
fn monorepo(base: &Path, manifest: &str) -> PathBuf {
    let repo = base.join("mono");
    init_repo(&repo);
    write(&repo, "tono.toml", manifest);
    write(&repo, "ir.json", IR_V1);
    commit_all(&repo, "first spec");
    repo
}

fn go_manifest(mirror: &Path, mode: &str) -> String {
    format!(
        "[target.go]\nout = \"dist/go\"\nsplit_repo = \"{}\"\n{mode}",
        mirror.display()
    )
}

/// Run `tono split <args> ir.json` in `repo`.
fn split(repo: &Path, args: &[&str]) -> std::process::Output {
    tono()
        .current_dir(repo)
        .arg("split")
        .args(args)
        .arg("ir.json")
        .output()
        .unwrap()
}

fn ok_or_stderr(out: &std::process::Output) {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

// --- snapshot mode (the default) --------------------------------------------

#[test]
fn snapshot_appends_one_build_commit_per_spec_change() {
    let base = scratch("snap-append");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, ""));

    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    // The mirror holds the freshly generated SDK at its root, with the
    // monorepo commit stamped in the message.
    let files = git_out(&mirror, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("spec/types.go"), "{files}");
    let head_short = git_out(&repo, &["rev-parse", "--short", "HEAD"]);
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert_eq!(log, format!("Generate from {head_short}"));

    // A spec change appends a second commit; the first stays its parent.
    let before = git_out(&mirror, &["rev-parse", "main"]);
    write(&repo, "ir.json", IR_V2);
    commit_all(&repo, "second spec");
    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    assert_eq!(git_out(&mirror, &["rev-parse", "main~1"]), before);
    let types = git_out(&mirror, &["show", "main:spec/types.go"]);
    assert!(types.contains("Currency"), "{types}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn snapshot_is_a_no_op_when_the_mirror_is_current() {
    let base = scratch("snap-noop");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, ""));

    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("up to date"), "{stdout}");
    assert_eq!(
        git_out(&mirror, &["rev-list", "--count", "main"]),
        "1",
        "an unchanged build must not add a commit"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn snapshot_keeps_user_files_and_prunes_stale_generated_ones() {
    let base = scratch("snap-prune");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, ""));

    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);

    // The user seeds the mirror with files tono does not generate. The branch
    // is named: the clone must not depend on the environment's default.
    let seed = base.join("seed");
    git(
        &base,
        &[
            "clone",
            "-q",
            "-b",
            "main",
            mirror.to_str().unwrap(),
            "seed",
        ],
    );
    git(&seed, &["config", "user.email", "t@example.com"]);
    git(&seed, &["config", "user.name", "t"]);
    write(&seed, "go.mod", "module example.com/demo\n");
    commit_all(&seed, "seed module manifest");
    git(&seed, &["push", "-q", "origin", "main"]);

    // Renaming the module moves the generated path: the old one must be
    // pruned from the mirror, the seeded go.mod must survive.
    let renamed = IR_V2
        .replace("\"spec\"", "\"billing\"")
        .replace("spec#", "billing#");
    write(&repo, "ir.json", &renamed);
    commit_all(&repo, "rename module");
    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    let files = git_out(&mirror, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("billing/types.go"), "{files}");
    assert!(files.contains("go.mod"), "{files}");
    assert!(!files.contains("spec/types.go"), "{files}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_failing_mirror_does_not_block_the_others() {
    let base = scratch("snap-besteffort");
    let mirror_go = bare_mirror(&base, "mirror-go");
    // The rust mirror does not exist: its push must fail, the go one must land.
    let manifest = format!(
        "[target.rust]\nout = \"dist/rust\"\nsplit_repo = \"{missing}\"\n\n\
         [target.go]\nout = \"dist/go\"\nsplit_repo = \"{go}\"\n",
        missing = base.join("no-such-mirror").display(),
        go = mirror_go.display()
    );
    let repo = monorepo(&base, &manifest);

    let out = split(&repo, &["--branch", "main"]);
    // The run fails overall (CI must notice) but only after every target ran.
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rust: split failed"), "{stderr}");
    assert!(stderr.contains("1 of 2 target(s): rust"), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("go: mirrored"), "{stdout}");
    assert!(
        git_out(&mirror_go, &["ls-tree", "-r", "--name-only", "main"]).contains("spec/types.go")
    );
    let _ = std::fs::remove_dir_all(&base);
}

/// The frontend path: with no IR argument, the project's own `.tono` sources
/// are compiled and mirrored. Skips when the frontend binary is not built.
#[test]
fn snapshot_compiles_the_project_sources_when_no_ir_is_given() {
    let frontend = match std::env::var_os("TONO_FRONTEND") {
        Some(p) => {
            let p = PathBuf::from(p);
            if !p.exists() {
                return;
            }
            p
        }
        None => {
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
            let p = root.join("_build/default/frontend/bin/tono_frontend.exe");
            if !p.exists() {
                eprintln!("skipping: frontend binary not built (set TONO_FRONTEND)");
                return;
            }
            p
        }
    };
    let base = scratch("snap-sources");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = base.join("mono");
    init_repo(&repo);
    write(&repo, "tono.toml", &go_manifest(&mirror, ""));
    write(
        &repo,
        "spec.tono",
        "pub struct charge {\n  amount: string\n}\n",
    );
    commit_all(&repo, "first spec");

    let out = tono()
        .env("TONO_FRONTEND", frontend)
        .current_dir(&repo)
        .args(["split", "--branch", "main"])
        .output()
        .unwrap();
    ok_or_stderr(&out);
    let files = git_out(&mirror, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("spec/types.go"), "{files}");
    let _ = std::fs::remove_dir_all(&base);
}

// --- subtree mode (the opt-in) ----------------------------------------------

/// Commit a fresh `tono gen` run, the state subtree mode projects.
fn gen_and_commit(repo: &Path, msg: &str) {
    let out = tono()
        .current_dir(repo)
        .args(["gen", "ir.json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    commit_all(repo, msg);
}

#[test]
fn subtree_projects_the_committed_history_with_only_its_own_commits() {
    let base = scratch("sub-mirror");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, "split_mode = \"subtree\"\n"));
    gen_and_commit(&repo, "first sdk drop");
    write(&repo, "README.md", "root only\n");
    commit_all(&repo, "root readme");
    write(&repo, "ir.json", IR_V2);
    gen_and_commit(&repo, "second sdk drop");

    let out = split(&repo, &["--branch", "main", "--ref", "HEAD"]);
    ok_or_stderr(&out);
    // The mirror's history is the subtree's own: both sdk drops, not the
    // root-only commit, with the dist/go contents at the root.
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert!(log.contains("first sdk drop"), "{log}");
    assert!(log.contains("second sdk drop"), "{log}");
    assert!(!log.contains("root readme"), "{log}");
    let files = git_out(&mirror, &["ls-tree", "-r", "--name-only", "main"]);
    assert!(files.contains("spec/types.go"), "{files}");
    assert!(!files.contains("dist"), "{files}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn subtree_refuses_to_project_a_stale_committed_sdk() {
    let base = scratch("sub-drift");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, "split_mode = \"subtree\"\n"));
    gen_and_commit(&repo, "first sdk drop");
    // The spec moves on but nobody regenerates: the gate must catch it.
    write(&repo, "ir.json", IR_V2);
    commit_all(&repo, "spec change without regen");

    let out = split(&repo, &["--branch", "main", "--ref", "HEAD"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("stale"), "{stderr}");
    assert!(
        git_out(&mirror, &["branch", "--format=%(refname:short)"]).is_empty(),
        "nothing may reach the mirror"
    );
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn subtree_default_ref_is_asked_from_the_remote_when_the_symref_is_absent() {
    let base = scratch("sub-default");
    let mirror = bare_mirror(&base, "mirror-go");
    let server = monorepo(&base, &go_manifest(&mirror, "split_mode = \"subtree\"\n"));
    gen_and_commit(&server, "sdk drop");
    // A default branch that is neither main nor master proves the name is
    // resolved, not guessed.
    git(&server, &["branch", "-q", "-M", "trunk"]);
    // A CI-shaped checkout: init + fetch + detach, which records no
    // origin/HEAD symref, so the remote itself must be asked.
    let work = base.join("work");
    init_repo(&work);
    git(
        &work,
        &["remote", "add", "origin", server.to_str().unwrap()],
    );
    git(&work, &["fetch", "-q", "origin"]);
    git(&work, &["checkout", "-q", "--detach", "origin/trunk"]);

    let out = split(&work, &["--branch", "main"]);
    ok_or_stderr(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("origin/trunk"), "{stdout}");
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert!(log.contains("sdk drop"), "{log}");
    let _ = std::fs::remove_dir_all(&base);
}

// --- shared surface ----------------------------------------------------------

#[test]
fn a_manifest_without_split_repo_is_a_no_op() {
    let base = scratch("noop");
    let repo = monorepo(&base, "[target.go]\nout = \"dist/go\"\n");

    let out = split(&repo, &["--branch", "main"]);
    ok_or_stderr(&out);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nothing to split"), "{stdout}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_missing_branch_flag_is_a_clear_error() {
    let base = scratch("nobranch");
    let repo = monorepo(&base, "[target.go]\nout = \"dist/go\"\n");

    let out = split(&repo, &[]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs --branch"), "{stderr}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_invalid_branch_name_is_rejected_before_any_push() {
    let base = scratch("badbranch");
    let mirror = bare_mirror(&base, "mirror-go");
    let repo = monorepo(&base, &go_manifest(&mirror, ""));

    let out = split(&repo, &["--branch", "..bad"]);
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("invalid --branch"), "{stderr}");
    // Rejected up front: the mirror never saw a push.
    assert_eq!(
        git_out(&mirror, &["branch", "--format=%(refname:short)"]),
        ""
    );
    let _ = std::fs::remove_dir_all(&base);
}

// Every IR fixture literal in this file embeds a bare version number; a
// stale one fails with a decode error far from this assertion. Catch it
// here instead: the same bump every past IR version change forgot.
#[test]
fn ir_fixtures_use_the_current_ir_version() {
    let expected = format!("\"tono_ir_version\":{}", tono_backend::ir::TONO_IR_VERSION);
    for (name, fixture) in [("IR_V1", IR_V1), ("IR_V2", IR_V2)] {
        assert!(
            fixture.contains(&expected),
            "stale tono_ir_version in {name}: {fixture}"
        );
    }
}

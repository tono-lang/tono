//! End-to-end checks of `tono split`: mirroring a target's `out/` subtree into
//! its own repository.
//!
//! Each test builds a throwaway monorepo with committed generated output and
//! bare local repositories as mirrors, then runs the command and inspects the
//! mirrors with plain git. Local paths stand in for the GitHub remotes; the
//! push plumbing is identical.

use std::path::{Path, PathBuf};
use std::process::Command;

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

/// Create a bare repository to act as a mirror remote and return its path.
fn bare_mirror(base: &Path, name: &str) -> PathBuf {
    let dir = base.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "--bare"]);
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

/// Build the standard monorepo: a manifest whose typescript target mirrors to
/// `mirror-ts`, three commits of which only the first and third touch `out/ts`.
fn monorepo(base: &Path, manifest: &str) -> PathBuf {
    let repo = base.join("mono");
    init_repo(&repo);
    write(&repo, "tono.toml", manifest);
    write(&repo, "out/ts/index.ts", "export const v = 1;\n");
    commit_all(&repo, "first sdk drop");
    write(&repo, "README.md", "root only\n");
    commit_all(&repo, "root readme");
    write(&repo, "out/ts/index.ts", "export const v = 2;\n");
    commit_all(&repo, "second sdk drop");
    repo
}

#[test]
fn the_subtree_is_mirrored_with_only_its_own_history() {
    let base = scratch("mirror");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "main", "--ref", "HEAD"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The mirror's main holds the subtree contents at its root, no out/ prefix.
    let files = git_out(&mirror, &["ls-tree", "--name-only", "main"]);
    assert_eq!(files, "index.ts");
    // History is the subtree's own: both sdk drops, not the root-only commit.
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert_eq!(log, "second sdk drop\nfirst sdk drop");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_second_run_is_stable_and_updates_the_mirror() {
    let base = scratch("rerun");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);

    let split = |repo: &Path| {
        tono()
            .current_dir(repo)
            .args(["split", "--branch", "main", "--ref", "HEAD"])
            .output()
            .unwrap()
    };
    let out = split(&repo);
    assert!(out.status.success());
    // A new subtree commit lands on the next run; the split is deterministic,
    // so the force-push fast-forwards the mirror instead of rewriting it.
    let before = git_out(&mirror, &["rev-parse", "main"]);
    write(&repo, "out/ts/index.ts", "export const v = 3;\n");
    commit_all(&repo, "third sdk drop");
    let out = split(&repo);
    assert!(out.status.success());
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert_eq!(log, "third sdk drop\nsecond sdk drop\nfirst sdk drop");
    assert_eq!(git_out(&mirror, &["rev-parse", "main~1"]), before);
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_manifest_without_split_repo_is_a_no_op() {
    let base = scratch("noop");
    let repo = monorepo(&base, "[target.typescript]\nout = \"out/ts\"\n");

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "main"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("nothing to split"), "{stdout}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_missing_branch_flag_is_a_clear_error() {
    let base = scratch("nobranch");
    let repo = monorepo(&base, "[target.typescript]\nout = \"out/ts\"\n");

    let out = tono().current_dir(&repo).arg("split").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("needs --branch"), "{stderr}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_failing_mirror_does_not_block_the_others() {
    let base = scratch("besteffort");
    let mirror_ts = bare_mirror(&base, "mirror-ts");
    // The rust mirror does not exist: its push must fail, the ts one must land.
    let manifest = format!(
        "[target.rust]\nout = \"out/rust\"\nsplit_repo = \"{missing}\"\n\n\
         [target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{ts}\"\n",
        missing = base.join("no-such-mirror").display(),
        ts = mirror_ts.display()
    );
    let repo = monorepo(&base, &manifest);
    write(&repo, "out/rust/lib.rs", "pub fn v() -> u32 { 1 }\n");
    commit_all(&repo, "rust sdk drop");

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "main", "--ref", "HEAD"])
        .output()
        .unwrap();
    // The run fails overall (CI must notice) but only after every target ran.
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("rust: split failed"), "{stderr}");
    assert!(stderr.contains("1 of 2 target(s): rust"), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("typescript: mirrored"), "{stdout}");
    let log = git_out(&mirror_ts, &["log", "--format=%s", "main"]);
    assert!(log.contains("second sdk drop"), "{log}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_default_ref_is_the_remote_default_branch_not_the_checkout() {
    let base = scratch("default-clone");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let server = monorepo(&base, &manifest);
    // A plain clone records the server's default branch as origin/HEAD.
    let work = base.join("work");
    git(&base, &["clone", "-q", server.to_str().unwrap(), "work"]);
    git(&work, &["config", "user.email", "t@example.com"]);
    git(&work, &["config", "user.name", "t"]);
    // Stray local work on another branch must not leak into the mirror.
    git(&work, &["checkout", "-q", "-b", "wip"]);
    write(&work, "out/ts/index.ts", "export const v = 99;\n");
    commit_all(&work, "stray work");

    let out = tono()
        .current_dir(&work)
        .args(["split", "--branch", "main"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert!(log.contains("second sdk drop"), "{log}");
    assert!(!log.contains("stray work"), "{log}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn the_default_ref_is_asked_from_the_remote_when_the_symref_is_absent() {
    let base = scratch("default-fetch");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let server = monorepo(&base, &manifest);
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

    let out = tono()
        .current_dir(&work)
        .args(["split", "--branch", "main"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("origin/trunk"), "{stdout}");
    let log = git_out(&mirror, &["log", "--format=%s", "main"]);
    assert!(log.contains("second sdk drop"), "{log}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn no_origin_and_no_ref_is_a_clear_error() {
    let base = scratch("default-none");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "main"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("pass --ref"), "{stderr}");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn a_side_branch_cut_lands_on_its_own_mirror_branch() {
    let base = scratch("branch");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);
    // A client-specific prerelease branch diverging from the mainline.
    git(&repo, &["checkout", "-q", "-b", "feat/acme-pilot"]);
    write(&repo, "out/ts/index.ts", "export const v = \"acme\";\n");
    commit_all(&repo, "acme pilot cut");

    let out = tono()
        .current_dir(&repo)
        .args([
            "split",
            "--ref",
            "feat/acme-pilot",
            "--branch",
            "alpha-acme",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The cut lands on its own mirror branch; main is never created.
    let log = git_out(&mirror, &["log", "--format=%s", "alpha-acme"]);
    assert!(log.contains("acme pilot cut"), "{log}");
    let branches = git_out(&mirror, &["branch", "--format=%(refname:short)"]);
    assert_eq!(branches, "alpha-acme");
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn an_invalid_branch_name_is_rejected_before_any_push() {
    let base = scratch("badbranch");
    let mirror = bare_mirror(&base, "mirror-ts");
    let manifest = format!(
        "[target.typescript]\nout = \"out/ts\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "..bad"])
        .output()
        .unwrap();
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

#[test]
fn an_out_dir_never_committed_is_reported_per_target() {
    let base = scratch("missing-out");
    let mirror = bare_mirror(&base, "mirror-go");
    let manifest = format!(
        "[target.go]\nout = \"out/go\"\nsplit_repo = \"{}\"\n",
        mirror.display()
    );
    let repo = monorepo(&base, &manifest);

    let out = tono()
        .current_dir(&repo)
        .args(["split", "--branch", "main", "--ref", "HEAD"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("go: split failed"), "{stderr}");
    let _ = std::fs::remove_dir_all(&base);
}

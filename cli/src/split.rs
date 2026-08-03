//! `tono split`: mirror each target's generated SDK into its own read-only
//! repository.
//!
//! The monorepo stays the single source of truth and the split always starts
//! from a fresh build: the project's sources are compiled and generated first,
//! so a mirror can never receive an SDK older than the spec it stands for.
//! How the result reaches the mirror is the target's `split_mode`:
//!
//! - `snapshot` (default): the freshly generated files become one commit
//!   appended to the mirror branch. Nothing needs to be committed in the
//!   monorepo and no history is rewritten, so it works in any repository.
//! - `subtree`: the committed history of the target's `out` directory is
//!   projected with `git subtree split` and force-pushed, for projects that
//!   commit the generated SDK and want the mirror to carry its real history.
//!   The fresh build doubles as a drift gate: a stale committed SDK fails the
//!   split instead of being mirrored.
//!
//! Only targets that opt in via `split_repo` participate. Each target is split
//! independently so one broken mirror cannot hold the others back; failures
//! are reported together at the end. The command never tags: versioning the
//! mirror belongs to whatever release process invokes it.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::{is_generated, TargetKind};
use tono_backend::config::{self as manifest, SplitMode};
use tono_backend::ir::decode_model;

use crate::gen;

/// Run `tono split --branch <name> [--config <tono.toml>] [--ref <committish>]`.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut split_ref: Option<String> = None;
    let mut branch: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => config_path = Some(crate::flag_value(args, &mut i, "--config")?),
            // The monorepo commit subtree targets project. Defaults to the
            // repository's default branch (the remote's HEAD), not the current
            // checkout, so a split from a stray working branch has to be asked
            // for. Snapshot targets always build the current sources instead.
            "--ref" => split_ref = Some(crate::flag_value(args, &mut i, "--ref")?),
            // The mirror branch the projection lands on. Required: a one-off
            // cut (an alpha for one client, from any monorepo state) can land
            // on its own mirror branch and leave main alone, so the caller
            // always says where the changes go.
            "--branch" => branch = Some(crate::flag_value(args, &mut i, "--branch")?),
            other => return Err(format!("unexpected argument: {other}\n{}", crate::USAGE)),
        }
        i += 1;
    }
    let branch = branch.ok_or_else(|| {
        format!(
            "split needs --branch <name>: the mirror branch the projection lands on\n{}",
            crate::USAGE
        )
    })?;

    let manifest_file = match config_path {
        Some(path) => PathBuf::from(path),
        None => crate::discover_manifest()?,
    };
    let base = manifest_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let cfg = manifest::Config::load(&manifest_file)?;

    let targets: Vec<_> = cfg
        .targets
        .iter()
        .filter_map(|t| t.split_repo.as_deref().map(|repo| (t, repo)))
        .collect();
    if targets.is_empty() {
        println!("nothing to split: no target sets split_repo");
        return Ok(());
    }

    let root = PathBuf::from(git(&base, &["rev-parse", "--show-toplevel"])?);
    // One bad branch name would fail every push with git's least helpful
    // error; reject it once, up front.
    git(&root, &["check-ref-format", "--branch", &branch])
        .map_err(|_| format!("invalid --branch name: '{branch}'"))?;

    let any_subtree = targets
        .iter()
        .any(|(t, _)| t.split_mode == SplitMode::Subtree);
    // Subtree-only prerequisites: projecting history needs all of it, and a
    // ref to project. Snapshot targets need neither.
    if any_subtree && git(&root, &["rev-parse", "--is-shallow-repository"])? == "true" {
        return Err("split needs the full history: the clone is shallow (fetch with --unshallow, or checkout with fetch-depth: 0 in CI)".into());
    }
    let split_ref = match (split_ref, any_subtree) {
        (Some(r), true) => Some(r),
        (None, true) => {
            let default = default_split_ref(&root)?;
            println!("splitting {default} (the default branch; pass --ref to override)");
            Some(default)
        }
        (given, false) => {
            if given.is_some() {
                eprintln!("note: --ref is ignored, every mirror here uses snapshot mode (the current sources are built and pushed)");
            }
            None
        }
    };

    // One build feeds every target: snapshot pushes it, subtree gates on it.
    // A spec that does not compile stops the whole run; that is not a
    // per-mirror failure, there is nothing correct to mirror.
    let model = decode_model(&gen::compile_sources(&base.join(&cfg.project.root))?)?;
    let run = SplitRun {
        root: &root,
        base: &base,
        branch: &branch,
        model: &model,
        provenance: &provenance(&root),
    };

    let mut failed: Vec<&str> = Vec::new();
    for (target, repo) in &targets {
        let lang = target.kind.dir();
        let outcome = match target.split_mode {
            SplitMode::Snapshot => snapshot_one(&run, target, repo),
            SplitMode::Subtree => subtree_one(
                &run,
                target,
                repo,
                split_ref.as_deref().expect("subtree targets resolve a ref"),
            ),
        };
        match outcome {
            Ok(status) => println!("{lang}: {status}"),
            Err(e) => {
                eprintln!("{lang}: split failed: {e}");
                failed.push(lang);
            }
        }
    }

    if failed.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "split failed for {} of {} target(s): {}",
            failed.len(),
            targets.len(),
            failed.join(", ")
        ))
    }
}

// --- snapshot mode ----------------------------------------------------------

/// What one invocation shares across every target it splits: the repository,
/// the manifest's directory, the destination branch, and the fresh build.
struct SplitRun<'a> {
    root: &'a Path,
    base: &'a Path,
    branch: &'a str,
    model: &'a tono_backend::ir::Model,
    provenance: &'a str,
}

/// Append the fresh build as one commit on the mirror's `branch`. The mirror
/// branch is fetched and extended, never rewritten: a push rejection means
/// something else wrote to the mirror, which is exactly what should surface.
/// Files the mirror carries that tono did not generate (a `go.mod`, a README,
/// a LICENSE) are left as they are; stale generated files are pruned by the
/// banner they carry.
fn snapshot_one(
    run: &SplitRun,
    target: &manifest::ResolvedTarget,
    repo: &str,
) -> Result<String, String> {
    let (branch, provenance) = (run.branch, run.provenance);
    let outputs = gen::target_outputs(run.model, target)?;
    let url = remote_url(repo);

    let tmp = std::env::temp_dir().join(format!(
        "tono-split-{}-{}",
        std::process::id(),
        target.kind.dir()
    ));
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    git(&tmp, &["init", "-q", "-b", branch])?;
    git(&tmp, &["remote", "add", "origin", &url])?;
    // A missing branch is simply a new mirror line; it is born here. (A
    // missing repository also lands in this arm, and the push below reports
    // it with git's own words.)
    if git(&tmp, &["fetch", "-q", "origin", branch]).is_ok() {
        git(&tmp, &["checkout", "-q", "-B", branch, "FETCH_HEAD"])?;
    }

    let expected: BTreeSet<PathBuf> = outputs.iter().map(|(rel, _)| rel.clone()).collect();
    prune_stale_generated(&tmp, &tmp, &expected)?;
    for (rel, text) in &outputs {
        gen::write_generated(&tmp.join(rel), target.kind, text)?;
    }

    git(&tmp, &["add", "-A"])?;
    if git(&tmp, &["diff", "--cached", "--quiet"]).is_ok() {
        let _ = fs::remove_dir_all(&tmp);
        return Ok(format!("up to date with {url}"));
    }
    let (name, email) = committer_identity(run.root);
    let name_arg = format!("user.name={name}");
    let email_arg = format!("user.email={email}");
    let message = format!("Generate from {provenance}");
    git(
        &tmp,
        &[
            "-c", &name_arg, "-c", &email_arg, "commit", "-q", "-m", &message,
        ],
    )?;
    let refspec = format!("{branch}:refs/heads/{branch}");
    git(&tmp, &["push", "-q", "origin", &refspec]).map_err(|e| {
        format!("{e}\nthe mirror is expected to change only through this command; reconcile or delete its '{branch}' branch and rerun")
    })?;
    let sha = git(&tmp, &["rev-parse", "--short", "HEAD"])?;
    let _ = fs::remove_dir_all(&tmp);
    Ok(format!("mirrored to {url} ({sha})"))
}

/// Delete generated files (identified by their banner) that this build no
/// longer produces, leaving everything the user owns untouched.
fn prune_stale_generated(
    top: &Path,
    dir: &Path,
    expected: &BTreeSet<PathBuf>,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        if path.file_name().is_some_and(|n| n == ".git") {
            continue;
        }
        if path.is_dir() {
            prune_stale_generated(top, &path, expected)?;
            continue;
        }
        let rel = path.strip_prefix(top).unwrap_or(&path).to_path_buf();
        if expected.contains(&rel) {
            continue;
        }
        if fs::read_to_string(&path).is_ok_and(|text| is_generated(&text)) {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// The monorepo state a snapshot commit records as its origin.
fn provenance(root: &Path) -> String {
    match git(root, &["rev-parse", "--short", "HEAD"]) {
        Ok(sha) => {
            let dirty = git(root, &["status", "--porcelain"]).is_ok_and(|s| !s.is_empty());
            if dirty {
                format!("{sha} (dirty)")
            } else {
                sha
            }
        }
        Err(_) => "uncommitted sources".to_string(),
    }
}

/// The identity mirror commits are written under: the monorepo's configured
/// committer when there is one, a neutral fallback otherwise (CI runners
/// often configure none).
fn committer_identity(root: &Path) -> (String, String) {
    let name = git(root, &["config", "user.name"])
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tono".to_string());
    let email = git(root, &["config", "user.email"])
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "tono@localhost".to_string());
    (name, email)
}

// --- subtree mode -----------------------------------------------------------

/// Project the committed history of the target's `out` directory at
/// `split_ref` and force-push the projected head to the mirror's `branch`.
fn subtree_one(
    run: &SplitRun,
    target: &manifest::ResolvedTarget,
    repo: &str,
    split_ref: &str,
) -> Result<String, String> {
    let (root, branch) = (run.root, run.branch);
    let prefix = subtree_prefix(root, run.base, &target.out)?;
    drift_gate(root, target, split_ref, &prefix, run.model)?;
    // The projected head is the last stdout line; earlier lines are progress.
    let sha = git(root, &["subtree", "split", "--prefix", &prefix, split_ref])?
        .lines()
        .last()
        .unwrap_or_default()
        .to_string();
    let url = remote_url(repo);
    // Force: the mirror is a projection, never a place history accumulates on
    // its own, so the split result is authoritative on every push.
    let refspec = format!("{sha}:refs/heads/{branch}");
    git(root, &["push", "--force", &url, &refspec])?;
    Ok(format!("mirrored to {url} ({sha})"))
}

/// Refuse to project a committed SDK that no longer matches the sources. The
/// gate only fires when the ref being split is the commit the working tree
/// stands on: that is the "I changed the spec and forgot to regenerate" case.
/// Projecting an older ref on purpose is left alone; its consistency was this
/// same gate's job when it was the tip.
fn drift_gate(
    root: &Path,
    target: &manifest::ResolvedTarget,
    split_ref: &str,
    prefix: &str,
    model: &tono_backend::ir::Model,
) -> Result<(), String> {
    let ref_sha = git(root, &["rev-parse", &format!("{split_ref}^{{commit}}")])?;
    let head_sha = git(root, &["rev-parse", "HEAD"])?;
    if ref_sha != head_sha {
        return Ok(());
    }

    let stale = |detail: String| {
        format!("the committed SDK is stale ({detail}); run tono gen and commit before splitting")
    };
    let outputs = gen::target_outputs(model, target)?;
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for (rel, text) in &outputs {
        let path = format!("{prefix}/{}", rel.display());
        expected.insert(path.clone());
        // The TypeScript package.json is merged with the user's manifest at
        // write time, so only its presence is checked here.
        let merged_on_write = target.kind == TargetKind::TypeScript
            && rel.file_name().is_some_and(|n| n == "package.json");
        let committed = git_raw(root, &["show", &format!("{split_ref}:{path}")])
            .map_err(|_| stale(format!("{path} is not committed")))?;
        if !merged_on_write && committed != *text {
            return Err(stale(format!("{path} differs from a fresh build")));
        }
    }
    // A generated file that is committed but no longer produced (a renamed or
    // deleted module) would live on in the mirror; only banner-carrying files
    // are tono's to flag.
    let listed = git(
        root,
        &["ls-tree", "-r", "--name-only", split_ref, "--", prefix],
    )?;
    for path in listed.lines() {
        if expected.contains(path) {
            continue;
        }
        let text = git_raw(root, &["show", &format!("{split_ref}:{path}")]).unwrap_or_default();
        if is_generated(&text) {
            return Err(stale(format!("{path} is no longer generated")));
        }
    }
    Ok(())
}

// --- shared plumbing --------------------------------------------------------

/// The default `--ref` for subtree targets: the repository's default branch,
/// the pure-git notion every host serves as the remote's `HEAD`. A plain clone
/// records it as the `origin/HEAD` symref; a fetch-built checkout (CI) does
/// not, so the remote is asked directly. The remote-tracking ref is preferred
/// over a local branch of the same name: the mirror should project what the
/// server holds, not a possibly stale local checkout.
fn default_split_ref(root: &Path) -> Result<String, String> {
    let name = match git(root, &["symbolic-ref", "refs/remotes/origin/HEAD"]) {
        Ok(symref) => symref
            .strip_prefix("refs/remotes/origin/")
            .map(str::to_string)
            .ok_or_else(|| format!("unexpected origin/HEAD symref: {symref}"))?,
        Err(_) => remote_head_name(root)?,
    };
    for candidate in [
        format!("refs/remotes/origin/{name}"),
        format!("refs/heads/{name}"),
    ] {
        if git(root, &["rev-parse", "--verify", "--quiet", &candidate]).is_ok() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "the default branch '{name}' is not available locally; fetch it or pass --ref"
    ))
}

/// Ask the `origin` remote which branch its `HEAD` points at.
fn remote_head_name(root: &Path) -> Result<String, String> {
    let out = git(root, &["ls-remote", "--symref", "origin", "HEAD"])
        .map_err(|e| format!("cannot resolve the default branch ({e}); pass --ref"))?;
    out.lines()
        .find_map(|line| {
            line.strip_prefix("ref: refs/heads/")
                .and_then(|rest| rest.strip_suffix("\tHEAD"))
        })
        .map(str::to_string)
        .ok_or_else(|| "origin did not report a default branch; pass --ref".to_string())
}

/// Expand the `owner/name` GitHub shorthand into a pushable URL; anything that
/// already names a protocol, host, or path is passed to git verbatim.
fn remote_url(spec: &str) -> String {
    let shorthand = !spec.contains("://")
        && !spec.contains('@')
        && !spec.starts_with('/')
        && !spec.starts_with('.')
        && matches!(
            spec.split('/').collect::<Vec<_>>().as_slice(),
            [owner, name] if !owner.is_empty() && !name.is_empty()
        );
    if shorthand {
        format!("https://github.com/{spec}.git")
    } else {
        spec.to_string()
    }
}

/// The target's `out` directory as a repo-root-relative prefix, the shape
/// `git subtree split --prefix` expects. `out` resolves against the manifest's
/// directory, which need not be the repository root.
fn subtree_prefix(root: &Path, base: &Path, out: &Path) -> Result<String, String> {
    let abs = base.join(out);
    let abs = abs
        .canonicalize()
        .map_err(|e| format!("{}: {e}", abs.display()))?;
    let root = root
        .canonicalize()
        .map_err(|e| format!("{}: {e}", root.display()))?;
    let rel = abs.strip_prefix(&root).map_err(|_| {
        format!(
            "out '{}' is outside the repository at {}",
            abs.display(),
            root.display()
        )
    })?;
    if rel.as_os_str().is_empty() {
        return Err("out is the repository root itself; nothing to split".into());
    }
    Ok(rel.to_string_lossy().into_owned())
}

/// Run git in `dir`, returning trimmed stdout or the trimmed stderr as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    Ok(git_raw(dir, args)?.trim().to_string())
}

/// Run git in `dir`, returning stdout verbatim (file contents compare exact).
fn git_raw(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_name_shorthand_expands_to_github() {
        assert_eq!(
            remote_url("acme/payments-go"),
            "https://github.com/acme/payments-go.git"
        );
    }

    #[test]
    fn urls_paths_and_ssh_remotes_pass_verbatim() {
        for spec in [
            "https://github.com/acme/payments-go.git",
            "git@github.com:acme/payments-go.git",
            "ssh://git@host/acme/payments-go.git",
            "/srv/mirrors/payments-go.git",
            "./mirrors/payments-go.git",
            "srv/mirrors/payments-go.git",
        ] {
            assert_eq!(remote_url(spec), spec, "{spec}");
        }
    }

    #[test]
    fn prefix_is_relative_to_the_repo_root() {
        let root = std::env::temp_dir().join(format!("tono-split-prefix-{}", std::process::id()));
        let out = root.join("packages").join("dist").join("go");
        std::fs::create_dir_all(&out).unwrap();
        let prefix = subtree_prefix(&root, &root.join("packages"), Path::new("dist/go")).unwrap();
        assert_eq!(prefix, "packages/dist/go");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_out_outside_the_repo_is_an_error() {
        let base = std::env::temp_dir().join(format!("tono-split-outside-{}", std::process::id()));
        let root = base.join("repo");
        let elsewhere = base.join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let err = subtree_prefix(&root, &base, Path::new("elsewhere")).unwrap_err();
        assert!(err.contains("outside the repository"), "{err}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn an_out_equal_to_the_repo_root_is_an_error() {
        let root = std::env::temp_dir().join(format!("tono-split-root-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let err = subtree_prefix(&root, &root, Path::new(".")).unwrap_err();
        assert!(err.contains("repository root"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_generated_files_are_pruned_and_user_files_kept() {
        let top = std::env::temp_dir().join(format!("tono-split-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&top);
        std::fs::create_dir_all(top.join("sub")).unwrap();
        // A stale generated file, a user file, and a still-expected file.
        std::fs::write(
            top.join("sub/old.go"),
            "// Code generated by tono. DO NOT EDIT.\n",
        )
        .unwrap();
        std::fs::write(top.join("go.mod"), "module demo\n").unwrap();
        std::fs::write(
            top.join("keep.go"),
            "// Code generated by tono. DO NOT EDIT.\n",
        )
        .unwrap();
        let expected: BTreeSet<PathBuf> = [PathBuf::from("keep.go")].into();
        prune_stale_generated(&top, &top, &expected).unwrap();
        assert!(!top.join("sub/old.go").exists());
        assert!(top.join("go.mod").exists());
        assert!(top.join("keep.go").exists());
        let _ = std::fs::remove_dir_all(&top);
    }
}

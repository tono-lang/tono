//! `tono split`: mirror each target's committed `out/` subtree into its own
//! read-only repository.
//!
//! The monorepo stays the single source of truth; a mirror is a projection of
//! one target's `out` directory, rebuilt from history with `git subtree split`
//! and force-pushed to the repository named by that target's `split_repo`.
//! Only targets that opt in via `split_repo` participate; a manifest without
//! the key makes this command a no-op. Each target is split independently so
//! one broken mirror (a missing repository, a revoked credential) cannot hold
//! the others back; failures are reported together at the end.
//!
//! The release flow stays the user's: the command moves a projection to a
//! named mirror branch, nothing more. It never invents a destination (the
//! mirror branch is a required argument) and it never tags; versioning the
//! mirror belongs to whatever release process invokes it.

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::config as manifest;

/// Run `tono split --branch <name> [--config <tono.toml>] [--ref <committish>]`.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut split_ref: Option<String> = None;
    let mut branch: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => config_path = Some(crate::flag_value(args, &mut i, "--config")?),
            // The monorepo commit to project. Defaults to the repository's
            // default branch (the remote's HEAD), not the current checkout,
            // so a split from a stray working branch has to be asked for.
            "--ref" => split_ref = Some(crate::flag_value(args, &mut i, "--ref")?),
            // The mirror branch the projection lands on. Required: a one-off
            // cut (an alpha for one client, from any monorepo branch) can
            // land on its own mirror branch and leave main alone, so the
            // caller always says where the changes go.
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
    // A shallow clone would make the subtree rebuild from a truncated history
    // and the force-push would overwrite the mirror with it; refuse up front
    // rather than quietly rewriting every mirror.
    if git(&root, &["rev-parse", "--is-shallow-repository"])? == "true" {
        return Err("split needs the full history: the clone is shallow (fetch with --unshallow, or checkout with fetch-depth: 0 in CI)".into());
    }
    let split_ref = match split_ref {
        Some(r) => r,
        None => {
            let default = default_split_ref(&root)?;
            println!("splitting {default} (the default branch; pass --ref to override)");
            default
        }
    };

    let mut failed: Vec<&str> = Vec::new();
    for (target, repo) in &targets {
        let lang = target.kind.dir();
        match split_one(&root, &base, target, repo, &split_ref, &branch) {
            Ok(sha) => println!("{lang}: mirrored to {} ({sha})", remote_url(repo)),
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

/// The default `--ref`: the repository's default branch, the pure-git notion
/// every host serves as the remote's `HEAD`. A plain clone records it as the
/// `origin/HEAD` symref; a fetch-built checkout (CI) does not, so the remote
/// is asked directly. The remote-tracking ref is preferred over a local branch
/// of the same name: the mirror should project what the server holds, not a
/// possibly stale local checkout.
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

/// Split one target's subtree at `split_ref` and force-push it to its mirror:
/// the projected head becomes the mirror's `branch`.
fn split_one(
    root: &Path,
    base: &Path,
    target: &manifest::ResolvedTarget,
    repo: &str,
    split_ref: &str,
    branch: &str,
) -> Result<String, String> {
    let prefix = subtree_prefix(root, base, &target.out)?;
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
    Ok(sha)
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
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
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
        let out = root.join("packages").join("out").join("go");
        std::fs::create_dir_all(&out).unwrap();
        let prefix = subtree_prefix(&root, &root.join("packages"), Path::new("out/go")).unwrap();
        assert_eq!(prefix, "packages/out/go");
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
}

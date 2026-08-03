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

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::config as manifest;

/// Run `tono split [--config <tono.toml>] [--ref <committish>] [--tag <name>]`.
pub fn run(args: &[String]) -> Result<(), String> {
    let mut config_path: Option<String> = None;
    let mut split_ref = "HEAD".to_string();
    let mut tag: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--config" => config_path = Some(crate::flag_value(args, &mut i, "--config")?),
            // The monorepo commit to project (a release tag in CI, HEAD locally).
            "--ref" => split_ref = crate::flag_value(args, &mut i, "--ref")?,
            // Also stamp the split head with this tag on every mirror, so a
            // mirror release is addressable the way registries expect (Go
            // modules resolve versions straight from the mirror's tags).
            "--tag" => tag = Some(crate::flag_value(args, &mut i, "--tag")?),
            other => return Err(format!("unexpected argument: {other}\n{}", crate::USAGE)),
        }
        i += 1;
    }

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
    // A shallow clone would make the subtree rebuild from a truncated history
    // and the force-push would overwrite the mirror with it; refuse up front
    // rather than quietly rewriting every mirror.
    if git(&root, &["rev-parse", "--is-shallow-repository"])? == "true" {
        return Err("split needs the full history: the clone is shallow (fetch with --unshallow, or checkout with fetch-depth: 0 in CI)".into());
    }

    let mut failed: Vec<&str> = Vec::new();
    for (target, repo) in &targets {
        let lang = target.kind.dir();
        match split_one(&root, &base, target, repo, &split_ref, tag.as_deref()) {
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

/// Split one target's subtree at `split_ref` and force-push it to its mirror:
/// the projected head becomes the mirror's `main`, plus `tag` when given.
fn split_one(
    root: &Path,
    base: &Path,
    target: &manifest::ResolvedTarget,
    repo: &str,
    split_ref: &str,
    tag: Option<&str>,
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
    let mut refspecs = vec![format!("{sha}:refs/heads/main")];
    if let Some(tag) = tag {
        refspecs.push(format!("{sha}:refs/tags/{tag}"));
    }
    let mut args = vec!["push", "--force", &url];
    args.extend(refspecs.iter().map(String::as_str));
    git(root, &args)?;
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

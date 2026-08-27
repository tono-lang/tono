//! Where each language's library is resolved from, and which file pins it.
//!
//! The index is built from the tree the generated SDK builds in (the
//! manifest target's `out` directory), because that is where the library
//! resolves the way the SDK will see it: the Go module's `replace`, the
//! `node_modules` beside the package, the crate the `Cargo.toml` depends on.
//! The lockfile of that tree is what makes the index current: a version
//! bump the manifest does not spell (a transitive update, a `replace` to a
//! newer checkout) changes the lockfile and invalidates the index with it.

use std::path::{Path, PathBuf};

use tono_backend::codegen::TargetKind;
use tono_backend::config::Config;

use super::format::Lockfile;

/// The manifest target's `out` directory for a language, joined to the
/// manifest's own directory; `None` when the manifest declares no such
/// target.
pub(crate) fn root_for(cfg: &Config, manifest_dir: &Path, lang: &str) -> Option<PathBuf> {
    let kind = match lang {
        "go" => TargetKind::Go,
        "ts" => TargetKind::TypeScript,
        "rust" => TargetKind::Rust,
        _ => return None,
    };
    cfg.targets
        .iter()
        .find(|t| t.kind == kind)
        .map(|t| manifest_dir.join(&t.out))
}

/// The first ancestor of `root` (itself included) holding `name`.
fn ancestor_with(root: &Path, name: &str) -> Option<PathBuf> {
    root.ancestors().map(|d| d.join(name)).find(|p| p.is_file())
}

/// The lockfile that pins the library for `lang` under `root`, digested as
/// it is now. The path is recorded even when the file is absent (a Go module
/// with no dependency yet has a `go.mod` but no `go.sum`), so that its
/// appearance later reads as a change.
pub(crate) fn lockfile_for(lang: &str, root: &Path) -> Lockfile {
    let path = match lang {
        "go" => ancestor_with(root, "go.mod").map(|m| m.with_file_name("go.sum")),
        "ts" => ancestor_with(root, "package-lock.json")
            .or_else(|| ancestor_with(root, "node_modules/.package-lock.json")),
        "rust" => ancestor_with(root, "Cargo.lock"),
        _ => None,
    };
    match path {
        Some(p) => Lockfile::at(&p),
        None => Lockfile::none(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::format::NO_LOCKFILE;

    fn tmp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tono-index-roots-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn roots_come_from_the_manifest_targets() {
        let cfg = Config::from_toml_str(
            "[project]\nname = \"p\"\n[target.go]\nout = \"sdk/go\"\n[target.typescript]\nout = \"sdk/ts\"\n",
        )
        .unwrap();
        let base = Path::new("/proj");
        assert_eq!(
            root_for(&cfg, base, "go"),
            Some(PathBuf::from("/proj/sdk/go"))
        );
        assert_eq!(
            root_for(&cfg, base, "ts"),
            Some(PathBuf::from("/proj/sdk/ts"))
        );
        assert_eq!(root_for(&cfg, base, "rust"), None);
        assert_eq!(root_for(&cfg, base, "zig"), None);
    }

    #[test]
    fn go_pins_on_the_go_sum_beside_the_nearest_go_mod() {
        let dir = tmp("go");
        write(&dir.join("go.mod"), "module m\n");
        write(&dir.join("go.sum"), "foobar");
        let root = dir.join("sdk").join("go");
        std::fs::create_dir_all(&root).unwrap();
        let lock = lockfile_for("go", &root);
        assert!(lock.path.ends_with("go.sum"), "{}", lock.path);
        assert_eq!(lock.digest, "85944171f73967e8");
    }

    #[test]
    fn go_records_the_expected_go_sum_when_it_is_absent() {
        let dir = tmp("go-nosum");
        write(&dir.join("go.mod"), "module m\n");
        let lock = lockfile_for("go", &dir);
        assert!(lock.path.ends_with("go.sum"), "{}", lock.path);
        assert_eq!(lock.digest, NO_LOCKFILE);
    }

    #[test]
    fn ts_prefers_the_package_lock_then_the_hidden_one_in_node_modules() {
        let dir = tmp("ts");
        write(&dir.join("node_modules/.package-lock.json"), "a");
        let hidden = lockfile_for("ts", &dir);
        assert!(
            hidden.path.ends_with(".package-lock.json"),
            "{}",
            hidden.path
        );
        assert_eq!(hidden.digest, "af63dc4c8601ec8c");
        write(&dir.join("package-lock.json"), "foobar");
        let visible = lockfile_for("ts", &dir.join("deep"));
        assert!(
            visible.path.ends_with("/package-lock.json"),
            "{}",
            visible.path
        );
        assert_eq!(visible.digest, "85944171f73967e8");
    }

    #[test]
    fn rust_pins_on_the_cargo_lock_above_the_root() {
        let dir = tmp("rust");
        write(&dir.join("Cargo.lock"), "foobar");
        let lock = lockfile_for("rust", &dir.join("sdk"));
        assert!(lock.path.ends_with("Cargo.lock"), "{}", lock.path);
        assert_eq!(lock.digest, "85944171f73967e8");
    }

    #[test]
    fn no_candidate_at_all_is_the_none_lockfile() {
        let dir = tmp("none");
        assert_eq!(lockfile_for("rust", &dir), Lockfile::none());
        assert_eq!(lockfile_for("go", &dir), Lockfile::none());
        assert_eq!(lockfile_for("ts", &dir), Lockfile::none());
        assert_eq!(lockfile_for("zig", &dir), Lockfile::none());
    }
}

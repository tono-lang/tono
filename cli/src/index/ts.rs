//! The TypeScript extractor's Rust side: where the compiler API is looked
//! for and how the helper runs. The reading itself is `helpers/extract.cjs`,
//! run by the consumer's own `node` inside their tree.

use std::path::{Path, PathBuf};

use tono_backend::codegen::verify::Scratch;

use super::{run_helper, Outcome};

const HELPER: &str = include_str!("helpers/extract.cjs");

/// The helper's name in the scratch directory. The `.cjs` extension is
/// load-bearing: the scratch sits under the SDK's `package.json`, which
/// declares `"type": "module"`, and node reads a `.js` there as an ES
/// module where `require` does not exist.
const HELPER_FILE: &str = "extract.cjs";

/// The environment variable naming a `typescript` package directory (or its
/// `lib/typescript.js`) to load the compiler API from, for a tree that has
/// none beside the library.
pub(crate) const API_ENV: &str = "TONO_TYPESCRIPT";

/// Where the compiler API may be, nearest first: the `typescript` package
/// or the `typescript-api` alias in a `node_modules` above the root (the
/// native compiler shipped as `typescript` 7 has no `lib/typescript.js`, so
/// the alias is how a tree keeps both), then the directory [`API_ENV`]
/// names. Only files that exist are candidates.
pub(crate) fn api_candidates(root: &Path) -> Vec<PathBuf> {
    api_candidates_with(root, std::env::var_os(API_ENV).map(PathBuf::from))
}

/// [`api_candidates`] with the environment's answer passed in, so the rule
/// is tested without touching the process environment.
fn api_candidates_with(root: &Path, env: Option<PathBuf>) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|d| {
            ["typescript", "typescript-api"]
                .into_iter()
                .map(move |name| d.join("node_modules").join(name).join("lib/typescript.js"))
        })
        .filter(|p| p.is_file())
        .collect();
    if let Some(env) = env {
        let file = if env.is_dir() {
            env.join("lib/typescript.js")
        } else {
            env
        };
        if file.is_file() {
            found.push(file);
        }
    }
    found
}

/// The symbols of `package`, resolved from `root` with the compiler API at
/// the first of `api` that loads.
pub(crate) fn extract_with(root: &Path, package: &str, api: &[PathBuf]) -> Result<Outcome, String> {
    if api.is_empty() {
        return Ok(Outcome::Skipped(format!(
            "no TypeScript compiler API beside the library (install typescript 5 or the typescript-api alias in the tree, or set {API_ENV})"
        )));
    }
    let scratch = Scratch::create(root, "index-ts").map_err(|e| e.to_string())?;
    std::fs::write(scratch.dir.join(HELPER_FILE), HELPER).map_err(|e| e.to_string())?;
    let root = root.to_string_lossy().into_owned();
    let mut args: Vec<&str> = vec![HELPER_FILE, &root, package];
    let api: Vec<String> = api
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    args.extend(api.iter().map(String::as_str));
    run_helper("node", &args, &scratch.dir, "node is not installed")
}

pub(crate) fn extract(root: &Path, package: &str) -> Result<Outcome, String> {
    extract_with(root, package, &api_candidates(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tono-index-ts-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn candidates_are_the_api_files_that_exist_nearest_first() {
        let dir = tmp("candidates");
        write(
            &dir.join("node_modules/typescript-api/lib/typescript.js"),
            "",
        );
        write(&dir.join("node_modules/typescript/package.json"), "{}");
        let root = dir.join("sdk/ts");
        write(&root.join("node_modules/typescript/lib/typescript.js"), "");
        let found = api_candidates(&root);
        assert_eq!(
            found,
            vec![
                root.join("node_modules/typescript/lib/typescript.js"),
                dir.join("node_modules/typescript-api/lib/typescript.js"),
            ]
        );
    }

    #[test]
    fn the_env_names_a_package_dir_or_the_file_itself_last() {
        let dir = tmp("env");
        let api = dir.join("api");
        write(&api.join("lib/typescript.js"), "");
        let root = dir.join("root");
        let found = api_candidates_with(&root, Some(api.clone()));
        let found_file = api_candidates_with(&root, Some(api.join("lib/typescript.js")));
        let found_none = api_candidates_with(&root, Some(dir.join("nowhere")));
        assert_eq!(found, vec![api.join("lib/typescript.js")]);
        assert_eq!(found_file, found);
        assert_eq!(found_none, Vec::<PathBuf>::new());
        // The public entry point walks the same candidates: none here.
        match extract(&dir.join("root"), "@example/gearbox").unwrap() {
            Outcome::Skipped(reason) => {
                assert!(reason.contains("no TypeScript compiler API"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn without_an_api_the_pair_is_skipped_before_anything_runs() {
        let dir = tmp("noapi");
        match extract_with(&dir, "@example/gearbox", &[]).unwrap() {
            Outcome::Skipped(reason) => {
                assert!(reason.contains("no TypeScript compiler API"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }

    /// A candidate that loads but is not the API is the same skip as none
    /// (when `node` runs the helper), or the missing toolchain (when it
    /// does not); the scratch directory is gone either way. The helper's
    /// reading of a real package is covered end to end by the `index`
    /// integration suite.
    #[test]
    fn a_candidate_that_is_not_the_api_is_skipped_and_the_scratch_removed() {
        let dir = tmp("fakeapi");
        let fake = dir.join("typescript.js");
        write(&fake, "module.exports = {};\n");
        match extract_with(&dir, "@example/gearbox", &[fake]).unwrap() {
            Outcome::Skipped(reason) => assert!(
                reason.contains("no TypeScript compiler API")
                    || reason.contains("node is not installed"),
                "{reason}"
            ),
            other => panic!("{other:?}"),
        }
        assert!(!std::fs::read_dir(&dir).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")));
    }
}

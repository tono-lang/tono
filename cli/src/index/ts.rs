//! The TypeScript extractor's Rust side: where the compiler API is looked
//! for and how the helper runs. The reading itself is `helpers/extract.js`,
//! run by the consumer's own `node` inside their tree.

use std::path::{Path, PathBuf};

use tono_backend::codegen::verify::Scratch;

use super::{run_helper, Outcome};

const HELPER: &str = include_str!("helpers/extract.js");

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
    let mut found: Vec<PathBuf> = root
        .ancestors()
        .flat_map(|d| {
            ["typescript", "typescript-api"]
                .into_iter()
                .map(move |name| d.join("node_modules").join(name).join("lib/typescript.js"))
        })
        .filter(|p| p.is_file())
        .collect();
    if let Some(env) = std::env::var_os(API_ENV) {
        let env = PathBuf::from(env);
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
    std::fs::write(scratch.dir.join("extract.js"), HELPER).map_err(|e| e.to_string())?;
    let root = root.to_string_lossy().into_owned();
    let mut args: Vec<&str> = vec!["extract.js", &root, package];
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
pub(crate) mod tests {
    use super::*;
    use crate::index::{MemberKind, SymbolKind};

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

    /// The compiler API this checkout can test with: the alias the codegen
    /// tests install, else what [`API_ENV`] names. `None` skips.
    pub(crate) fn test_api() -> Option<PathBuf> {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
        let alias = repo
            .join("backend/codegen-tests/typescript/node_modules/typescript-api/lib/typescript.js");
        if alias.is_file() {
            return Some(alias);
        }
        std::env::var_os(API_ENV)
            .map(PathBuf::from)
            .map(|p| {
                if p.is_dir() {
                    p.join("lib/typescript.js")
                } else {
                    p
                }
            })
            .filter(|p| p.is_file())
    }

    /// A tree whose `node_modules` holds a stand-in package with re-exports,
    /// overloads, a class with static and instance members, an interface,
    /// an enum and a namespace.
    pub(crate) fn fixture_root(dir: &Path) -> PathBuf {
        let root = dir.join("consumer-ts");
        let pkg = root.join("node_modules/@example/gearbox");
        write(
            &pkg.join("package.json"),
            "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"lib/index.d.ts\"}",
        );
        write(
            &pkg.join("lib/index.d.ts"),
            "export * from \"./core\";\n\
             export { Gauge as Meter } from \"./extra\";\n\
             /** Build a dial.\n *\n * Second paragraph. */\n\
             export declare function build(name: string): Dial;\n\
             export declare function build(name: string, size: number): Dial;\n\
             export type Size = \"s\" | \"m\";\n\
             export declare const VERSION: string;\n\
             export declare const make: (name: string) => Dial;\n\
             export default class Dial {\n  constructor(name: string);\n  static create(name: string): Dial;\n  private secret: number;\n  readonly name: string;\n  read(depth?: number): number;\n}\n\
             export interface Options { size?: Size; verbose: boolean }\n",
        );
        write(
            &pkg.join("lib/core.d.ts"),
            "export declare class Panel { open(): void; width: number }\n\
             export declare enum Mode { Fast, Slow }\n\
             export declare namespace util { function pad(s: string): string; namespace deep { const x: number } }\n",
        );
        write(
            &pkg.join("lib/extra.d.ts"),
            "export declare class Gauge { level(): number }\n",
        );
        root
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

    #[test]
    fn a_candidate_that_is_not_the_api_is_skipped_with_the_same_reason() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: node is not installed");
            return;
        }
        let dir = tmp("fakeapi");
        let fake = dir.join("typescript.js");
        write(&fake, "module.exports = {};\n");
        match extract_with(&dir, "@example/gearbox", &[fake]).unwrap() {
            Outcome::Skipped(reason) => {
                assert!(reason.contains("no TypeScript compiler API"), "{reason}")
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn the_helper_follows_re_exports_and_keeps_overloads_on_one_symbol() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: node is not installed");
            return;
        }
        let Some(api) = test_api() else {
            eprintln!("skipping: no TypeScript compiler API (npm install in backend/codegen-tests/typescript, or set {API_ENV})");
            return;
        };
        let dir = tmp("lib");
        let root = fixture_root(&dir);
        let (symbols, note) = match extract_with(&root, "@example/gearbox", &[api]).unwrap() {
            Outcome::Built { symbols, note } => (symbols, note),
            Outcome::Skipped(reason) => panic!("skipped: {reason}"),
        };
        assert_eq!(note, None);
        let mut names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "Meter", "Mode", "Options", "Panel", "Size", "VERSION", "build", "default", "make",
                "util"
            ]
        );
        let by = |n: &str| symbols.iter().find(|s| s.name == n).unwrap();
        let build = by("build");
        assert_eq!(build.kind, SymbolKind::Function);
        assert_eq!(
            build.signatures,
            vec!["(name: string): Dial", "(name: string, size: number): Dial"]
        );
        assert_eq!(build.doc, "Build a dial.");
        let dial = by("default");
        assert_eq!(dial.kind, SymbolKind::Class);
        assert_eq!(dial.signatures, vec!["(name: string): Dial"]);
        let members: Vec<(&str, MemberKind, bool)> = dial
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.kind, m.is_static))
            .collect();
        assert_eq!(
            members,
            vec![
                ("create", MemberKind::Method, true),
                ("name", MemberKind::Field, false),
                ("read", MemberKind::Method, false),
            ]
        );
        assert_eq!(by("Meter").kind, SymbolKind::Class);
        assert_eq!(by("Meter").members[0].name, "level");
        assert_eq!(by("Panel").members.len(), 2);
        assert_eq!(by("make").kind, SymbolKind::Function);
        assert_eq!(by("VERSION").kind, SymbolKind::Const);
        assert_eq!(by("VERSION").signatures, vec!["string"]);
        assert_eq!(by("Size").kind, SymbolKind::Type);
        assert_eq!(by("Size").signatures, vec!["\"s\" | \"m\""]);
        assert_eq!(by("Size").members, vec![]);
        let mode = by("Mode");
        assert_eq!(mode.kind, SymbolKind::Enum);
        let variants: Vec<&str> = mode.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(variants, vec!["Fast", "Slow"]);
        let util = by("util");
        assert_eq!(util.kind, SymbolKind::Namespace);
        let inner: Vec<(&str, MemberKind)> = util
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.kind))
            .collect();
        assert_eq!(
            inner,
            vec![("pad", MemberKind::Function), ("deep.x", MemberKind::Const)]
        );
        let options = by("Options");
        assert_eq!(options.kind, SymbolKind::Interface);
        assert_eq!(options.members[0].signatures, vec!["Size"]);
        // The scratch directory is gone with the extractor.
        assert!(!std::fs::read_dir(&root).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")));
    }

    #[test]
    fn a_package_that_does_not_resolve_is_skipped_with_the_reason() {
        if std::process::Command::new("node")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("skipping: node is not installed");
            return;
        }
        let Some(api) = test_api() else {
            eprintln!("skipping: no TypeScript compiler API");
            return;
        };
        let dir = tmp("unresolved");
        match extract_with(&dir, "@example/nowhere", &[api]).unwrap() {
            Outcome::Skipped(reason) => assert!(reason.contains("does not resolve"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }
}

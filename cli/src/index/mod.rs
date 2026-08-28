//! `tono index`: the symbols of every library an `ext` block binds, written
//! as one neutral JSON index per (ext, language) for the editor to complete
//! inside `#(...)` from.
//!
//! The command compiles the source (the frontend says which libraries the
//! file binds and how each language spells them), resolves each language's
//! library from the manifest target's `out` directory (the tree the
//! generated SDK builds in), runs that language's extractor once, and writes
//! the index under `.tono/index/` beside the manifest, keyed by everything
//! it was built from. The editor runs this command itself when it finds no
//! current index for a pair; it reads the file directly, so completion never
//! waits on a process.
//!
//! What the index says is a suggestion, never a verdict: the binding check
//! (`tono check`) is the one place a spelling is judged. An extractor that
//! misses a symbol makes a suggestion absent, and one that lists something
//! the library does not have makes a suggestion the check refuses. Neither
//! can make a wrong binding pass.

mod format;
mod go;
mod roots;
mod rust;
mod rust_exports;
mod rust_walk;
mod ts;

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tono_backend::config::{self as manifest, normalize_ext_lang};
use tono_backend::ir::{decode_model, Model};

use crate::frontend::Frontend;
use crate::{check, flag_value, USAGE};

#[cfg(test)]
pub(crate) use format::SymbolKind;
pub(crate) use format::{Index, Key, Symbol};

/// The parsed command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Args {
    pub path: Option<String>,
    pub config: Option<String>,
    pub json: bool,
    /// The (ext, lang) pairs to index; empty means every pair.
    pub only: Vec<(String, String)>,
}

pub(crate) fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--only" => {
                let value = flag_value(args, &mut i, "--only")?;
                let (ext, lang) = value
                    .split_once('=')
                    .ok_or_else(|| format!("--only expects <ext>=<lang>, got: {value}\n{USAGE}"))?;
                out.only
                    .push((ext.to_string(), normalize_ext_lang(lang).to_string()));
            }
            "--json" => out.json = true,
            "--config" => out.config = Some(flag_value(args, &mut i, "--config")?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag: {flag}\n{USAGE}")),
            p if out.path.is_none() => out.path = Some(p.to_string()),
            p => return Err(format!("unexpected extra argument: {p}\n{USAGE}")),
        }
        i += 1;
    }
    Ok(out)
}

/// One line of the report. The JSON form (`--json`) is this enum one object
/// per line, what the editor reads; the text form is its `Display`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub(crate) enum Line {
    Built {
        ext: String,
        lang: String,
        package: String,
        version: String,
        path: String,
        symbols: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The pair has no index and the reason is the user's to act on (no
    /// version pinned, no type source, no toolchain): the editor shows it.
    Skipped {
        ext: String,
        lang: String,
        reason: String,
    },
    /// The command itself could not run (the frontend rejected the source,
    /// no manifest): the message a text run prints as its error.
    Error { message: String },
}

impl std::fmt::Display for Line {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Line::Built {
                ext,
                lang,
                package,
                version,
                path,
                symbols,
                note,
            } => {
                write!(
                    f,
                    "built: {ext}/{lang} ({package} {version}): {symbols} symbols -> {path}"
                )?;
                if let Some(note) = note {
                    write!(f, " ({note})")?;
                }
                Ok(())
            }
            Line::Skipped { ext, lang, reason } => write!(f, "skipped: {ext}/{lang}: {reason}"),
            Line::Error { message } => write!(f, "{message}"),
        }
    }
}

pub(crate) fn render_lines(lines: &[Line]) -> String {
    lines.iter().map(|l| format!("{l}\n")).collect()
}

pub(crate) fn json_lines(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| {
            let mut s = serde_json::to_string(l).expect("a report line serializes");
            s.push('\n');
            s
        })
        .collect()
}

#[cfg(test)]
pub(crate) fn parse_json_lines(text: &str) -> Result<Vec<Line>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("report line {l:?}: {e}")))
        .collect()
}

/// One library in one language, as the source binds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pair {
    pub ext: String,
    pub lang: String,
    /// The library as the `.tono` spells it: the import path, the package
    /// name, the crate.
    pub package: String,
}

/// Every (ext, language) pair the model binds, in declaration order.
pub(crate) fn pairs_of(model: &Model) -> Vec<Pair> {
    model
        .modules
        .iter()
        .flat_map(|m| &m.ext_libs)
        .flat_map(|lib| {
            lib.langs.iter().map(|l| Pair {
                ext: lib.name.clone(),
                lang: normalize_ext_lang(&l.lang).to_string(),
                package: l.path.clone(),
            })
        })
        .collect()
}

/// The pairs `--only` keeps; every pair without it.
pub(crate) fn select(pairs: Vec<Pair>, only: &[(String, String)]) -> Vec<Pair> {
    if only.is_empty() {
        return pairs;
    }
    pairs
        .into_iter()
        .filter(|p| only.iter().any(|(e, l)| *e == p.ext && *l == p.lang))
        .collect()
}

/// What an extractor produced for a pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Outcome {
    Built {
        symbols: Vec<Symbol>,
        note: Option<String>,
    },
    Skipped(String),
}

/// The JSON a helper program prints: the symbols it found (with an optional
/// note), or the one reason it could not look. One shape for every helper,
/// so the Rust side of a new extractor is only how the helper is started.
#[derive(Deserialize)]
struct HelperOutput {
    #[serde(default)]
    skipped: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    symbols: Vec<Symbol>,
}

pub(crate) fn parse_helper_output(text: &str) -> Result<Outcome, String> {
    let out: HelperOutput = serde_json::from_str(text.trim())
        .map_err(|e| format!("the extractor printed something that is not its report: {e}"))?;
    Ok(match out.skipped {
        Some(reason) => Outcome::Skipped(reason),
        None => Outcome::Built {
            symbols: out.symbols,
            note: out.note,
        },
    })
}

/// The line of a failed helper's stderr that says what failed: the first
/// naming an error or a throw (node prints the stack and then its version
/// banner, which would be the last line), else the last non-empty line.
pub(crate) fn failure_line(stderr: &str) -> &str {
    let lines: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    lines
        .iter()
        .find(|l| l.contains("Error") || l.contains("error") || l.starts_with("throw "))
        .or(lines.last())
        .copied()
        .unwrap_or("printed no report")
}

/// Start a helper program and read its report. A program that cannot start
/// is the toolchain missing (a skip the user can act on); a program that
/// ran and printed no report failed, and its stderr says why.
pub(crate) fn run_helper(
    program: &str,
    args: &[&str],
    cwd: &Path,
    missing: &str,
) -> Result<Outcome, String> {
    let output = match Command::new(program).args(args).current_dir(cwd).output() {
        Ok(o) => o,
        Err(_) => return Ok(Outcome::Skipped(missing.to_string())),
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    match parse_helper_output(&stdout) {
        Ok(outcome) => Ok(outcome),
        Err(_) => Err(format!(
            "{program} extractor: {}",
            failure_line(&String::from_utf8_lossy(&output.stderr))
        )),
    }
}

/// Run the language's extractor over its library root.
fn build_pair(pair: &Pair, root: &Path, version: &str) -> Result<Outcome, String> {
    match pair.lang.as_str() {
        "go" => go::extract(root, &pair.package),
        "ts" => ts::extract(root, &pair.package),
        "rust" => rust::extract(root, &pair.package, version),
        other => Ok(Outcome::Skipped(format!("no {other} extractor yet"))),
    }
}

/// The extractor for a pair, as `index_model` calls it: injected so the
/// planning and the writing are tested without any toolchain.
type Builder<'a> = &'a dyn Fn(&Pair, &Path, &str) -> Result<Outcome, String>;

/// Index every selected pair of `model` under `manifest_dir`: the report
/// lines, in pair order. A pair the manifest does not pin, a target it does
/// not declare, or a root not on disk is a skip with the reason; a pair the
/// builder indexes is written beside the manifest under its key.
fn index_model(
    model: &Model,
    cfg: &manifest::Config,
    manifest_dir: &Path,
    only: &[(String, String)],
    build: Builder<'_>,
) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    for pair in select(pairs_of(model), only) {
        let skipped = |reason: String| Line::Skipped {
            ext: pair.ext.clone(),
            lang: pair.lang.clone(),
            reason,
        };
        let Some(version) = cfg
            .ext_versions
            .get(&pair.ext)
            .and_then(|langs| langs.get(&pair.lang))
        else {
            lines.push(skipped(format!(
                "no {} version pinned in [ext.{}] of {}",
                pair.lang,
                pair.ext,
                crate::MANIFEST_NAME
            )));
            continue;
        };
        let Some(root) = roots::root_for(cfg, manifest_dir, &pair.lang) else {
            lines.push(skipped(format!(
                "the manifest declares no {} target to resolve the library from",
                pair.lang
            )));
            continue;
        };
        if !root.is_dir() {
            lines.push(skipped(format!(
                "the library root {} does not exist (run tono gen first)",
                root.display()
            )));
            continue;
        }
        match build(&pair, &root, version)? {
            Outcome::Skipped(reason) => lines.push(skipped(reason)),
            Outcome::Built { symbols, note } => {
                let key = Key {
                    ext: pair.ext.clone(),
                    lang: pair.lang.clone(),
                    package: pair.package.clone(),
                    version: version.clone(),
                    lockfile: roots::lockfile_for(&pair.lang, &root),
                    format: format::FORMAT,
                };
                let index = Index::new(key, note.clone(), symbols);
                let path = format::index_path(manifest_dir, &pair.ext, &pair.lang);
                format::write(&index, &path)?;
                lines.push(Line::Built {
                    ext: pair.ext.clone(),
                    lang: pair.lang.clone(),
                    package: pair.package.clone(),
                    version: version.clone(),
                    path: path.to_string_lossy().into_owned(),
                    symbols: index.symbols.len(),
                    note,
                });
            }
        }
    }
    Ok(lines)
}

/// Index the pairs of `source`: the manifest above it, the source compiled
/// by the frontend (which says which libraries it binds), every pair built
/// by its language's extractor.
fn index_pairs(source: &Path, args: &Args) -> Result<Vec<Line>, String> {
    let manifest_path = check::manifest_for(source, args.config.as_deref()).ok_or_else(|| {
        format!(
            "no {} above {}; pass --config",
            crate::MANIFEST_NAME,
            source.display()
        )
    })?;
    let cfg = manifest::Config::load(&manifest_path)?;
    // Absolute from here on: the helpers run from a scratch directory of
    // their own and resolve the library from the root they are given, and
    // the index records paths the editor reads back from anywhere.
    let manifest_path = std::fs::canonicalize(&manifest_path).unwrap_or(manifest_path);
    let manifest_dir = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let ir = Frontend::from_env()
        .compile_as(source, None)
        .map_err(check::frontend_error)?;
    let model = decode_model(&ir)?;
    index_model(&model, &cfg, &manifest_dir, &args.only, &build_pair)
}

/// What a run prints and whether it succeeded. With `json` the lines go to
/// stdout as JSON, a failure of the run itself as an error line (the editor
/// reads one stream); without it the text goes to stderr and a failure is
/// the command's own error.
fn report(outcome: Result<Vec<Line>, String>, json: bool) -> Result<(String, bool), String> {
    if json {
        Ok(match outcome {
            Ok(lines) => (json_lines(&lines), true),
            Err(message) => (json_lines(&[Line::Error { message }]), false),
        })
    } else {
        outcome.map(|lines| (render_lines(&lines), true))
    }
}

/// `tono index`: one report line per (ext, language) pair. A skipped pair is
/// information (the editor shows why there is no completion for it), not a
/// failure; only the command failing to run at all exits non-zero.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args)?;
    let path = parsed
        .path
        .clone()
        .ok_or(format!("missing <file.tono>\n{USAGE}"))?;
    let (text, ok) = report(index_pairs(Path::new(&path), &parsed), parsed.json)?;
    if parsed.json {
        print!("{text}");
    } else {
        eprint!("{text}");
    }
    if ok {
        Ok(())
    } else {
        std::process::exit(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_path_only_and_json() {
        let parsed = parse_args(&strings(&[
            "a.tono",
            "--only",
            "gearbox=typescript",
            "--json",
            "--config",
            "x/tono.toml",
        ]))
        .unwrap();
        assert_eq!(parsed.path.as_deref(), Some("a.tono"));
        assert_eq!(parsed.only, vec![("gearbox".to_string(), "ts".to_string())]);
        assert!(parsed.json);
        assert_eq!(parsed.config.as_deref(), Some("x/tono.toml"));
    }

    #[test]
    fn rejects_a_bad_only_an_unknown_flag_and_an_extra_argument() {
        assert!(parse_args(&strings(&["a.tono", "--only", "gearbox"]))
            .unwrap_err()
            .contains("--only expects"));
        assert!(parse_args(&strings(&["a.tono", "--nope"]))
            .unwrap_err()
            .contains("unknown flag"));
        assert!(parse_args(&strings(&["a.tono", "b.tono"]))
            .unwrap_err()
            .contains("unexpected extra argument"));
        assert!(parse_args(&strings(&["a.tono", "--only"]))
            .unwrap_err()
            .contains("needs a value"));
    }

    #[test]
    fn lines_render_as_text_and_round_trip_as_json() {
        let lines = vec![
            Line::Built {
                ext: "gearbox".into(),
                lang: "go".into(),
                package: "example.test/gearbox".into(),
                version: "v0.0.0".into(),
                path: "/p/.tono/index/gearbox.go.json".into(),
                symbols: 2,
                note: Some("partial".into()),
            },
            Line::Skipped {
                ext: "gearbox".into(),
                lang: "rust".into(),
                reason: "no rust version pinned".into(),
            },
            Line::Error {
                message: "boom".into(),
            },
        ];
        assert_eq!(
            render_lines(&lines),
            "built: gearbox/go (example.test/gearbox v0.0.0): 2 symbols -> /p/.tono/index/gearbox.go.json (partial)\n\
             skipped: gearbox/rust: no rust version pinned\n\
             boom\n"
        );
        let json = json_lines(&lines);
        assert!(json.starts_with("{\"kind\":\"built\""), "{json}");
        assert_eq!(parse_json_lines(&json).unwrap(), lines);
        let bare = render_lines(&[Line::Built {
            ext: "g".into(),
            lang: "ts".into(),
            package: "@x/g".into(),
            version: "1".into(),
            path: "p".into(),
            symbols: 0,
            note: None,
        }]);
        assert_eq!(bare, "built: g/ts (@x/g 1): 0 symbols -> p\n");
    }

    #[test]
    fn select_keeps_only_the_named_pairs() {
        let pairs = vec![
            Pair {
                ext: "a".into(),
                lang: "go".into(),
                package: "p".into(),
            },
            Pair {
                ext: "a".into(),
                lang: "ts".into(),
                package: "q".into(),
            },
        ];
        assert_eq!(select(pairs.clone(), &[]), pairs);
        assert_eq!(
            select(pairs.clone(), &[("a".into(), "ts".into())]),
            vec![pairs[1].clone()]
        );
        assert_eq!(select(pairs, &[("b".into(), "go".into())]), vec![]);
    }

    #[test]
    fn pairs_of_reads_every_language_path_of_every_ext() {
        let ir = format!(
            r#"{{"tono_ir_version":{},"modules":[{{"name":"svc","ext_libs":[
                {{"name":"gearbox","langs":[{{"lang":"go","path":"example.test/gearbox"}},{{"lang":"typescript","path":"@example/gearbox"}}]}},
                {{"name":"lamp","langs":[{{"lang":"rust","path":"lamp"}}]}}]}}]}}"#,
            tono_backend::ir::TONO_IR_VERSION
        );
        let model = decode_model(&ir).unwrap();
        let pairs: Vec<(String, String, String)> = pairs_of(&model)
            .into_iter()
            .map(|p| (p.ext, p.lang, p.package))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("gearbox".into(), "go".into(), "example.test/gearbox".into()),
                ("gearbox".into(), "ts".into(), "@example/gearbox".into()),
                ("lamp".into(), "rust".into(), "lamp".into()),
            ]
        );
    }

    fn model(ext_libs: &str) -> Model {
        decode_model(&format!(
            r#"{{"tono_ir_version":{},"modules":[{{"name":"svc","ext_libs":[{ext_libs}]}}]}}"#,
            tono_backend::ir::TONO_IR_VERSION
        ))
        .unwrap()
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tono-index-mod-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const GEARBOX: &str = r#"{"name":"gearbox","langs":[{"lang":"go","path":"example.test/gearbox"},{"lang":"ts","path":"@example/gearbox"},{"lang":"rust","path":"gearbox"}]}"#;

    fn sample_symbols() -> Vec<Symbol> {
        vec![Symbol {
            name: "Open".into(),
            kind: SymbolKind::Function,
            signatures: vec!["func()".into()],
            doc: String::new(),
            members: vec![],
        }]
    }

    #[test]
    fn a_pair_is_skipped_before_the_builder_runs_when_the_project_cannot_place_it() {
        let dir = tmp("plan");
        std::fs::create_dir_all(dir.join("sdk/go")).unwrap();
        // go: pinned, target declared, root present; ts: pinned but no
        // target; rust: target declared but not pinned, and no root either.
        let cfg = manifest::Config::from_toml_str(
            "[project]\nname = \"svc\"\n[target.go]\nout = \"sdk/go\"\n[target.rust]\nout = \"sdk/rust\"\n[ext.gearbox]\ngo = \"v0.0.0\"\nts = \"1.0.0\"\n",
        )
        .unwrap();
        let calls = std::cell::RefCell::new(Vec::new());
        let build = |p: &Pair, root: &Path, version: &str| {
            calls
                .borrow_mut()
                .push((p.lang.clone(), root.to_path_buf(), version.to_string()));
            Ok(Outcome::Skipped("nothing here".into()))
        };
        let lines = index_model(&model(GEARBOX), &cfg, &dir, &[], &build).unwrap();
        let reasons: Vec<String> = lines
            .iter()
            .map(|l| match l {
                Line::Skipped { lang, reason, .. } => format!("{lang}: {reason}"),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(reasons[0], "go: nothing here");
        assert_eq!(
            reasons[1],
            "ts: the manifest declares no ts target to resolve the library from"
        );
        assert_eq!(
            reasons[2],
            "rust: no rust version pinned in [ext.gearbox] of tono.toml"
        );
        assert_eq!(
            calls.borrow().as_slice(),
            &[("go".to_string(), dir.join("sdk/go"), "v0.0.0".to_string())]
        );
        // A root the manifest names but that is not on disk.
        let cfg = manifest::Config::from_toml_str(
            "[project]\nname = \"svc\"\n[target.typescript]\nout = \"sdk/ts\"\n[ext.gearbox]\nts = \"1.0.0\"\n",
        )
        .unwrap();
        let lines = index_model(
            &model(GEARBOX),
            &cfg,
            &dir,
            &[("gearbox".into(), "ts".into())],
            &build,
        )
        .unwrap();
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            Line::Skipped { reason, .. } => {
                assert!(
                    reason.contains("does not exist (run tono gen first)"),
                    "{reason}"
                )
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_built_pair_is_written_under_its_key_and_reported() {
        let dir = tmp("built");
        std::fs::create_dir_all(dir.join("sdk/go")).unwrap();
        std::fs::write(dir.join("sdk/go/go.mod"), "module example.test/consumer\n").unwrap();
        std::fs::write(dir.join("sdk/go/go.sum"), "foobar").unwrap();
        let cfg = manifest::Config::from_toml_str(
            "[project]\nname = \"svc\"\n[target.go]\nout = \"sdk/go\"\n[ext.gearbox]\ngo = \"v0.0.0\"\n",
        )
        .unwrap();
        let build = |_: &Pair, _: &Path, _: &str| {
            Ok(Outcome::Built {
                symbols: sample_symbols(),
                note: Some("a note".into()),
            })
        };
        let lines = index_model(
            &model(GEARBOX),
            &cfg,
            &dir,
            &[("gearbox".into(), "go".into())],
            &build,
        )
        .unwrap();
        let path = format::index_path(&dir, "gearbox", "go");
        assert_eq!(
            lines,
            vec![Line::Built {
                ext: "gearbox".into(),
                lang: "go".into(),
                package: "example.test/gearbox".into(),
                version: "v0.0.0".into(),
                path: path.to_string_lossy().into_owned(),
                symbols: 1,
                note: Some("a note".into()),
            }]
        );
        let index = format::read(&path).unwrap();
        assert_eq!(index.key.package, "example.test/gearbox");
        assert_eq!(index.key.version, "v0.0.0");
        assert!(index.key.lockfile.path.ends_with("sdk/go/go.sum"));
        assert_eq!(index.key.lockfile.digest, "85944171f73967e8");
        assert_eq!(index.symbols, sample_symbols());
        // A builder that fails is the run failing, not a skip.
        let failing = |_: &Pair, _: &Path, _: &str| Err("helper crashed".to_string());
        let err = index_model(&model(GEARBOX), &cfg, &dir, &[], &failing).unwrap_err();
        assert_eq!(err, "helper crashed");
    }

    #[test]
    fn the_report_is_json_on_stdout_or_text_on_stderr() {
        let lines = vec![Line::Skipped {
            ext: "g".into(),
            lang: "go".into(),
            reason: "r".into(),
        }];
        assert_eq!(
            report(Ok(lines.clone()), true).unwrap(),
            (json_lines(&lines), true)
        );
        assert_eq!(
            report(Ok(lines.clone()), false).unwrap(),
            ("skipped: g/go: r\n".to_string(), true)
        );
        let (text, ok) = report(Err("boom".into()), true).unwrap();
        assert_eq!(text, "{\"kind\":\"error\",\"message\":\"boom\"}\n");
        assert!(!ok);
        assert_eq!(report(Err("boom".into()), false).unwrap_err(), "boom");
    }

    #[test]
    fn run_helper_tells_a_missing_program_from_a_failing_one() {
        let dir = tmp("helper");
        match run_helper("tono-no-such-helper-xyz", &[], &dir, "not installed").unwrap() {
            Outcome::Skipped(reason) => assert_eq!(reason, "not installed"),
            other => panic!("{other:?}"),
        }
        let err = run_helper(
            "sh",
            &[
                "-c",
                "echo not json; echo first >&2; echo last line >&2; exit 3",
            ],
            &dir,
            "not installed",
        )
        .unwrap_err();
        assert_eq!(err, "sh extractor: last line");
        // A node crash: the stack, the error, then the version banner.
        assert_eq!(
            failure_line(
                "/x/extract.cjs:1\nconst path = require(\"path\");\n\nReferenceError: require is not defined in ES module scope\n    at file:///x/extract.cjs:1:14\n\nNode.js v22.19.0\n"
            ),
            "ReferenceError: require is not defined in ES module scope"
        );
        assert_eq!(failure_line("throw err;\n^\nsomething\n"), "throw err;");
        assert_eq!(failure_line("  \n"), "printed no report");
        let err = run_helper("sh", &["-c", "exit 1"], &dir, "not installed").unwrap_err();
        assert_eq!(err, "sh extractor: printed no report");
        match run_helper("sh", &["-c", "echo '{\"skipped\":\"why\"}'"], &dir, "x").unwrap() {
            Outcome::Skipped(reason) => assert_eq!(reason, "why"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn build_pair_dispatches_by_language_and_refuses_an_unknown_one() {
        let dir = tmp("dispatch");
        let pair = |lang: &str| Pair {
            ext: "gearbox".into(),
            lang: lang.into(),
            package: "gearbox".into(),
        };
        let skip = |lang: &str| match build_pair(&pair(lang), &dir, "0.0.0").unwrap() {
            Outcome::Skipped(reason) => reason,
            other => panic!("{other:?}"),
        };
        assert!(skip("go").contains("no go.mod above"));
        assert!(skip("ts").contains("no TypeScript compiler API"));
        assert!(skip("rust").contains("no Cargo.toml"));
        assert_eq!(skip("zig"), "no zig extractor yet");
    }

    #[test]
    fn index_pairs_needs_a_manifest_and_the_frontend() {
        let dir = tmp("pairs");
        let source = dir.join("svc.tono");
        std::fs::write(
            &source,
            "ext gearbox {\n  go { #(example.test/gearbox) }\n}\n",
        )
        .unwrap();
        let err = index_pairs(&source, &Args::default()).unwrap_err();
        assert!(err.contains("no tono.toml above"), "{err}");
        std::fs::write(dir.join("tono.toml"), "[project]\nname = \"svc\"\n").unwrap();
        // With the frontend built this indexes (nothing to build: no
        // target); without it the frontend's absence is the error.
        match index_pairs(&source, &Args::default()) {
            Ok(lines) => assert!(matches!(lines[0], Line::Skipped { .. }), "{lines:?}"),
            Err(err) => assert!(
                err.contains("could not run") || err.contains("frontend"),
                "{err}"
            ),
        }
        // The command itself: a missing file argument, and a text run that
        // fails before printing.
        assert!(run(&[]).unwrap_err().contains("missing <file.tono>"));
        let err = run(&["/nonexistent-tono-project/x.tono".to_string()]).unwrap_err();
        assert!(err.contains("no tono.toml above"), "{err}");
    }

    #[test]
    fn helper_output_is_symbols_or_one_skip_reason() {
        let built = parse_helper_output(
            r#"{"symbols":[{"name":"Open","kind":"function","signatures":["func()"]}],"note":"n"}"#,
        )
        .unwrap();
        match built {
            Outcome::Built { symbols, note } => {
                assert_eq!(symbols[0].name, "Open");
                assert_eq!(symbols[0].kind, SymbolKind::Function);
                assert_eq!(note.as_deref(), Some("n"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            parse_helper_output(r#"{"skipped":"no source"}"#).unwrap(),
            Outcome::Skipped("no source".into())
        );
        assert!(parse_helper_output("panic: boom")
            .unwrap_err()
            .contains("not its report"));
        assert!(matches!(
            parse_helper_output("{}").unwrap(),
            Outcome::Built { symbols, note: None } if symbols.is_empty()
        ));
    }
}

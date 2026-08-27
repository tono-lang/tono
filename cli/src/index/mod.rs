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
mod ts;

use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use tono_backend::config::{self as manifest, normalize_ext_lang};
use tono_backend::ir::{decode_model, Model};

use crate::frontend::Frontend;
use crate::{check, flag_value, USAGE};

pub(crate) use format::{Index, Key, Symbol};
#[cfg(test)]
pub(crate) use format::{MemberKind, SymbolKind};

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

/// Start a helper program and read its report. A program that cannot start
/// is the toolchain missing (a skip the user can act on); a program that
/// ran and printed no report failed, and its last stderr line says why.
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
        Err(_) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .last()
                .unwrap_or("printed no report");
            Err(format!("{program} extractor: {last}"))
        }
    }
}

/// Run the language's extractor over its library root.
fn build_pair(pair: &Pair, root: &Path, version: &str) -> Result<Outcome, String> {
    let _ = version;
    match pair.lang.as_str() {
        "go" => go::extract(root, &pair.package),
        "ts" => ts::extract(root, &pair.package),
        other => Ok(Outcome::Skipped(format!("no {other} extractor yet"))),
    }
}

/// Index every selected pair of `source`: the report lines, in pair order.
fn index_pairs(source: &Path, args: &Args) -> Result<Vec<Line>, String> {
    let manifest_path = check::manifest_for(source, args.config.as_deref()).ok_or_else(|| {
        format!(
            "no {} above {}; pass --config",
            crate::MANIFEST_NAME,
            source.display()
        )
    })?;
    let cfg = manifest::Config::load(&manifest_path)?;
    let manifest_dir = manifest_path
        .parent()
        .unwrap_or(Path::new("."))
        .to_path_buf();
    let frontend = Frontend::from_env();
    let ir = frontend
        .compile_as(source, None)
        .map_err(check::frontend_error)?;
    let model = decode_model(&ir)?;
    let pairs = select(pairs_of(&model), &args.only);
    let mut lines = Vec::new();
    for pair in pairs {
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
        let Some(root) = roots::root_for(&cfg, &manifest_dir, &pair.lang) else {
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
        match build_pair(&pair, &root, version)? {
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
                let path = format::index_path(&manifest_dir, &pair.ext, &pair.lang);
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

/// `tono index`: one report line per (ext, language) pair. A skipped pair is
/// information (the editor shows why there is no completion for it), not a
/// failure; only the command failing to run at all exits non-zero. With
/// `--json` the lines go to stdout as JSON, an error included.
pub(crate) fn run(args: &[String]) -> Result<(), String> {
    let parsed = parse_args(args)?;
    let path = parsed
        .path
        .clone()
        .ok_or(format!("missing <file.tono>\n{USAGE}"))?;
    let outcome = index_pairs(Path::new(&path), &parsed);
    if parsed.json {
        let (lines, ok) = match outcome {
            Ok(lines) => (lines, true),
            Err(message) => (vec![Line::Error { message }], false),
        };
        print!("{}", json_lines(&lines));
        if ok {
            Ok(())
        } else {
            std::process::exit(1)
        }
    } else {
        let lines = outcome?;
        eprint!("{}", render_lines(&lines));
        Ok(())
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

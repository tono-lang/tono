//! The report of a `tono index` run: one line per (ext, language) pair,
//! as text for a terminal or as JSON lines for the editor. Two spellings of
//! the same sequence, so the editor reads exactly what the command prints.

use serde::{Deserialize, Serialize};

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

/// What a run prints and whether it succeeded. With `json` the lines go to
/// stdout as JSON, a failure of the run itself as an error line (the editor
/// reads one stream); without it the text goes to stderr and a failure is
/// the command's own error.
pub(crate) fn report(
    outcome: Result<Vec<Line>, String>,
    json: bool,
) -> Result<(String, bool), String> {
    if json {
        Ok(match outcome {
            Ok(lines) => (json_lines(&lines), true),
            Err(message) => (json_lines(&[Line::Error { message }]), false),
        })
    } else {
        outcome.map(|lines| (render_lines(&lines), true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

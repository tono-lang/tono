//! Syntax highlighting for the preview panes, via tree-sitter and each generated
//! language's official grammar.
//!
//! The input is plain text (tabs already expanded) and the output is a ratatui
//! [`Text`] with one styled span per highlight capture. Highlighting is cosmetic:
//! any failure (a query that will not compile, a highlighter error) falls back to
//! the plain text rather than surfacing an error, so the preview never breaks over
//! colors. The `.tono` source pane uses the tono grammar itself ([`source`]); the
//! generated pane uses the grammar of whichever target it shows ([`generated`]).

use std::sync::OnceLock;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

use tono_backend::codegen::TargetKind;

/// The capture names the theme styles, matched by prefix (so `function.method`
/// falls under `function`). Order is the [`THEME`] index.
const CAPTURES: [&str; 13] = [
    "attribute",
    "comment",
    "constant",
    "constructor",
    "function",
    "keyword",
    "module",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "type",
];

/// One style per capture in [`CAPTURES`], tuned for a dark terminal but readable
/// on light ones (no dim-on-dim pairs).
const THEME: [Style; 13] = [
    Style::new().fg(Color::Yellow),                            // attribute
    Style::new().fg(Color::DarkGray),                          // comment
    Style::new().fg(Color::LightYellow),                       // constant
    Style::new().fg(Color::Cyan),                              // constructor
    Style::new().fg(Color::Blue),                              // function
    Style::new().fg(Color::Magenta),                           // keyword
    Style::new().fg(Color::LightCyan),                         // module
    Style::new().fg(Color::LightYellow),                       // number
    Style::new(),                                              // operator
    Style::new().fg(Color::LightBlue),                         // property
    Style::new(),                                              // punctuation
    Style::new().fg(Color::Green),                             // string
    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD), // type
];

/// Highlight `text` as `target`'s language. Falls back to [`plain`] when the
/// grammar cannot highlight it (never an error).
pub fn generated(text: &str, target: TargetKind) -> Text<'static> {
    match config_for(target) {
        Some(config) => styled(text, config).unwrap_or_else(|| plain(text)),
        None => plain(text),
    }
}

/// Highlight `text` as tono source, for the left pane. Falls back to [`plain`]
/// like [`generated`] does.
pub fn source(text: &str) -> Text<'static> {
    static TONO: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    let config = TONO.get_or_init(|| {
        // The grammar tags comments `@comment @spell`; `@spell` is spell-check
        // metadata, not a color, and an unconfigured trailing capture suppresses
        // the configured one, so it is stripped rather than themed.
        let highlights = tree_sitter_tono::HIGHLIGHTS_QUERY.replace("@spell", "");
        build(tree_sitter_tono::LANGUAGE.into(), "tono", &highlights, "")
    });
    match config {
        Some(config) => styled(text, config).unwrap_or_else(|| plain(text)),
        None => plain(text),
    }
}

/// The un-highlighted rendering: the same [`Text`] shape, default style; the
/// fallback whenever a grammar cannot deliver.
pub fn plain(text: &str) -> Text<'static> {
    Text::from(
        text.lines()
            .map(|line| Line::raw(line.to_string()))
            .collect::<Vec<_>>(),
    )
}

/// The compiled highlight configuration for a target, built once per process:
/// query compilation is far too slow to redo per keystroke or per frame.
fn config_for(target: TargetKind) -> Option<&'static HighlightConfiguration> {
    static RUST: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    static GO: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    static TS: OnceLock<Option<HighlightConfiguration>> = OnceLock::new();
    match target {
        TargetKind::Rust => RUST.get_or_init(|| {
            build(
                tree_sitter_rust::LANGUAGE.into(),
                "rust",
                tree_sitter_rust::HIGHLIGHTS_QUERY,
                "",
            )
        }),
        TargetKind::Go => GO.get_or_init(|| {
            build(
                tree_sitter_go::LANGUAGE.into(),
                "go",
                tree_sitter_go::HIGHLIGHTS_QUERY,
                "",
            )
        }),
        TargetKind::TypeScript => TS.get_or_init(|| {
            // The TypeScript queries extend the JavaScript ones; the grammar crate
            // ships only the delta, so the two are concatenated (JS first, TS
            // refining it).
            let highlights = format!(
                "{}\n{}",
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY
            );
            build(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                "typescript",
                &highlights,
                tree_sitter_typescript::LOCALS_QUERY,
            )
        }),
    }
    .as_ref()
}

fn build(
    language: tree_sitter::Language,
    name: &str,
    highlights: &str,
    locals: &str,
) -> Option<HighlightConfiguration> {
    let mut config = HighlightConfiguration::new(language, name, highlights, "", locals).ok()?;
    config.configure(&CAPTURES);
    Some(config)
}

/// Run the highlighter and fold its event stream into styled lines. Events carry
/// byte ranges of `text`; a highlight span may cross newlines, so source chunks
/// are split on `\n` while the current style persists.
fn styled(text: &str, config: &HighlightConfiguration) -> Option<Text<'static>> {
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, text.as_bytes(), None, |_| None)
        .ok()?;

    let mut lines: Vec<Line> = vec![Line::default()];
    let mut stack: Vec<Style> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(capture) => stack.push(THEME[capture.0]),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let style = stack.last().copied().unwrap_or_default();
                for (i, part) in text[start..end].split('\n').enumerate() {
                    if i > 0 {
                        lines.push(Line::default());
                    }
                    if !part.is_empty() {
                        lines
                            .last_mut()
                            .expect("lines starts non-empty")
                            .spans
                            .push(Span::styled(part.to_string(), style));
                    }
                }
            }
        }
    }
    Some(Text::from(lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spans of one rendered line, as (content, has-color) pairs.
    fn line_spans(text: &Text, index: usize) -> Vec<(String, bool)> {
        text.lines[index]
            .spans
            .iter()
            .map(|s| (s.content.to_string(), s.style.fg.is_some()))
            .collect()
    }

    #[test]
    fn go_keywords_are_colored_and_content_is_intact() {
        let text = generated("type Card struct {\n    Last4 string\n}\n", TargetKind::Go);
        let flat: String = text
            .lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert_eq!(flat, "type Card struct {\n    Last4 string\n}\n\n");
        let first = line_spans(&text, 0);
        assert!(
            first
                .iter()
                .any(|(content, colored)| content == "type" && *colored),
            "expected a colored `type` keyword, got {first:?}"
        );
    }

    #[test]
    fn each_target_highlights_its_language() {
        let rust = generated("pub struct Charge;\n", TargetKind::Rust);
        let ts = generated(
            "export interface Charge { amount: string; }\n",
            TargetKind::TypeScript,
        );
        for (text, name) in [(&rust, "rust"), (&ts, "typescript")] {
            let colored = text
                .lines
                .iter()
                .flat_map(|l| &l.spans)
                .any(|s| s.style.fg.is_some());
            assert!(colored, "{name}: expected at least one colored span");
        }
    }

    #[test]
    fn tono_source_is_highlighted() {
        let text = source("// a comment\npub struct card { last4: string }\n");
        let colored: Vec<String> = text
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .filter(|s| s.style.fg.is_some())
            .map(|s| s.content.to_string())
            .collect();
        assert!(
            colored.iter().any(|c| c == "struct" || c == "pub"),
            "expected a colored tono keyword, got {colored:?}"
        );
        assert!(
            colored.iter().any(|c| c.contains("comment")),
            "expected the comment colored, got {colored:?}"
        );
    }

    #[test]
    fn plain_preserves_lines_without_styling() {
        let text = plain("a\nb\n");
        assert_eq!(text.lines.len(), 2);
        assert!(text
            .lines
            .iter()
            .flat_map(|l| &l.spans)
            .all(|s| s.style.fg.is_none()));
    }
}

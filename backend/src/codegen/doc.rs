//! Lowering `@doc` Markdown to each target's native documentation comment.
//!
//! The `@doc` content is CommonMark. JSDoc (TypeScript) and rustdoc (Rust) render
//! Markdown natively, so the content passes through verbatim, one comment line per
//! source line. godoc (Go) is not Markdown, so it is flattened to plain readable
//! text first (bold/code markers dropped, links unwrapped to `text (url)`, heading
//! hashes and code fences stripped). Each target's official formatter owns the
//! final layout; these helpers only produce syntactically valid comment lines.

/// rustdoc: one `///` line per content line, each prefixed with `indent` and
/// terminated with a newline so the documented item follows on the next line.
/// Empty content yields an empty string.
pub fn rustdoc(doc: &str, indent: &str) -> String {
    line_comment(doc, indent, "///")
}

/// godoc: one `//` line per content line, after flattening the Markdown to plain
/// text (godoc does not render Markdown). Empty content yields an empty string.
pub fn godoc(doc: &str, indent: &str) -> String {
    line_comment(&flatten_markdown(doc), indent, "//")
}

/// One `marker` line per content line. A blank content line becomes a bare marker
/// (no trailing space) so the comment stays clean under the target's formatter.
fn line_comment(text: &str, indent: &str, marker: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for line in text.split('\n') {
        if line.is_empty() {
            out.push_str(indent);
            out.push_str(marker);
            out.push('\n');
        } else {
            out.push_str(indent);
            out.push_str(marker);
            out.push(' ');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// A JSDoc block combining the `@doc` Markdown and an optional `@deprecated` tag,
/// indented and newline-terminated, or empty when neither is present. A single
/// content line collapses to the one-line `/** ... */` form (so a bare
/// `@deprecated` stays `/** @deprecated */`); anything longer becomes a multi-line
/// block. A `*/` in the content is defused so it cannot close the comment early.
pub fn jsdoc_block(doc: Option<&str>, deprecated: Option<&str>, indent: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    if let Some(d) = doc {
        for line in d.split('\n') {
            lines.push(defuse(line));
        }
    }
    if let Some(reason) = deprecated {
        if reason.is_empty() {
            lines.push("@deprecated".to_string());
        } else {
            lines.push(format!(
                "@deprecated {}",
                defuse(&reason.replace('\n', " "))
            ));
        }
    }
    match lines.as_slice() {
        [] => String::new(),
        [single] => format!("{indent}/** {single} */\n"),
        many => {
            let mut out = format!("{indent}/**\n");
            for line in many {
                if line.is_empty() {
                    out.push_str(&format!("{indent} *\n"));
                } else {
                    out.push_str(&format!("{indent} * {line}\n"));
                }
            }
            out.push_str(&format!("{indent} */\n"));
            out
        }
    }
}

/// Neutralize a `*/` so an embedded sequence cannot close a JSDoc/block comment.
fn defuse(s: &str) -> String {
    s.replace("*/", "* /")
}

/// Flatten CommonMark to plain readable text for godoc. This is deliberately
/// lightweight (no Markdown dependency): it drops bold markers and code spans,
/// unwraps links to `text (url)`, strips heading hashes, and removes code-fence
/// delimiters (keeping the fenced content). Single `*`/`_` are left untouched so
/// prose and identifiers (snake_case) survive intact.
pub fn flatten_markdown(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_fence = false;
    for raw in s.split('\n') {
        let trimmed = raw.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push(raw.to_string());
        } else {
            out.push(flatten_inline(&strip_heading(raw)));
        }
    }
    out.join("\n")
}

/// Remove a leading ATX heading marker (`#` through `######` followed by spaces).
fn strip_heading(line: &str) -> String {
    let after_indent = line.trim_start_matches([' ', '\t']);
    let hashes = after_indent.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&hashes) {
        let rest = &after_indent[hashes..];
        if rest.starts_with([' ', '\t']) || rest.is_empty() {
            return rest.trim_start().to_string();
        }
    }
    line.to_string()
}

/// Drop bold markers and code spans, then unwrap links.
fn flatten_inline(s: &str) -> String {
    let no_bold = s.replace("**", "");
    let no_code = no_bold.replace('`', "");
    flatten_links(&no_code)
}

/// Rewrite `[text](url)` to `text (url)` (or just `text` when the URL is empty),
/// leaving any malformed bracket run untouched.
fn flatten_links(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            if let Some(close) = find(&chars, i + 1, ']') {
                if close + 1 < chars.len() && chars[close + 1] == '(' {
                    if let Some(paren) = find(&chars, close + 2, ')') {
                        let text: String = chars[i + 1..close].iter().collect();
                        let url: String = chars[close + 2..paren].iter().collect();
                        out.push_str(&text);
                        if !url.is_empty() {
                            out.push_str(" (");
                            out.push_str(&url);
                            out.push(')');
                        }
                        i = paren + 1;
                        continue;
                    }
                }
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Index of the next `target` at or after `from`, or `None`.
fn find(chars: &[char], from: usize, target: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rustdoc_prefixes_each_line() {
        assert_eq!(rustdoc("one\ntwo", ""), "/// one\n/// two\n");
        // A blank line stays a bare `///` with no trailing space.
        assert_eq!(rustdoc("a\n\nb", "    "), "    /// a\n    ///\n    /// b\n");
        assert_eq!(rustdoc("", ""), "");
    }

    #[test]
    fn godoc_flattens_before_prefixing() {
        assert_eq!(godoc("Creates a **charge**.", ""), "// Creates a charge.\n");
    }

    #[test]
    fn jsdoc_single_line_stays_inline() {
        assert_eq!(
            jsdoc_block(Some("A charge."), None, ""),
            "/** A charge. */\n"
        );
        assert_eq!(jsdoc_block(None, Some(""), ""), "/** @deprecated */\n");
        assert_eq!(
            jsdoc_block(None, Some("use v2"), ""),
            "/** @deprecated use v2 */\n"
        );
        assert_eq!(jsdoc_block(None, None, ""), "");
    }

    #[test]
    fn jsdoc_doc_and_deprecation_combine_into_one_block() {
        assert_eq!(
            jsdoc_block(Some("A charge."), Some("use v2"), ""),
            "/**\n * A charge.\n * @deprecated use v2\n */\n"
        );
    }

    #[test]
    fn jsdoc_multiline_doc_becomes_a_block_with_blank_lines() {
        assert_eq!(
            jsdoc_block(Some("Title\n\nBody."), None, ""),
            "/**\n * Title\n *\n * Body.\n */\n"
        );
    }

    #[test]
    fn jsdoc_defuses_a_comment_terminator() {
        assert_eq!(
            jsdoc_block(Some("ends */ here"), None, ""),
            "/** ends * / here */\n"
        );
    }

    #[test]
    fn flatten_strips_bold_code_and_headings() {
        assert_eq!(flatten_markdown("# Title"), "Title");
        assert_eq!(
            flatten_markdown("a **bold** and `code` word"),
            "a bold and code word"
        );
        // A snake_case identifier keeps its underscores.
        assert_eq!(flatten_markdown("see amount_cents"), "see amount_cents");
    }

    #[test]
    fn flatten_unwraps_links_and_drops_fences() {
        assert_eq!(
            flatten_markdown("see [the docs](http://x)"),
            "see the docs (http://x)"
        );
        assert_eq!(flatten_markdown("[bare]()"), "bare");
        assert_eq!(
            flatten_markdown("before\n```\ncode line\n```\nafter"),
            "before\ncode line\nafter"
        );
    }
}

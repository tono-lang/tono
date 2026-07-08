//! Naming & casing: a snake_case canonical name into the idiomatic casing for a
//! symbol kind.
//!
//! The canonical name is always snake_case, so word boundaries are unambiguous
//! (`split('_')`). Each symbol kind maps to a configurable [`CaseStyle`] with a
//! built-in initialism set, so an acronym like `HTTP`/`URL`/`ID` is re-upcased
//! rather than title-cased. Two orthogonal overrides ride different axes and are
//! handled elsewhere in the pipeline: `@rename(lang)` replaces the in-code
//! identifier (the `rename` argument here, which bypasses casing entirely and is
//! purely cosmetic), and `@wire` replaces the serialization key (a field/variant
//! node concern that never touches the identifier).

use std::collections::{BTreeSet, HashMap};

use crate::codegen::symbol::SymbolKind;

/// A casing style for an identifier built from snake_case words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseStyle {
    /// `PaymentMethod`.
    Pascal,
    /// `paymentMethod`.
    Camel,
    /// `payment_method`.
    Snake,
    /// `PAYMENT_METHOD`.
    ScreamingSnake,
}

/// Initialisms recognized by default, so they are emitted upper-cased in word
/// positions (`HTTPURLID`) instead of title-cased (`HttpUrlId`).
const DEFAULT_ACRONYMS: &[&str] = &[
    "http", "https", "url", "uri", "id", "uuid", "api", "json", "xml", "html", "io", "ip", "db",
    "sdk", "ttl", "sql",
];

/// Per-symbol-kind casing rules plus the acronym set. The engine consumes an
/// already-built config; parsing the user's naming config lives elsewhere.
pub struct CasingConfig {
    default: CaseStyle,
    overrides: HashMap<SymbolKind, CaseStyle>,
    acronyms: BTreeSet<String>,
}

impl CasingConfig {
    /// A config that applies `default` to every kind, seeded with the built-in
    /// initialism set.
    pub fn new(default: CaseStyle) -> Self {
        Self {
            default,
            overrides: HashMap::new(),
            acronyms: DEFAULT_ACRONYMS.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    /// Override the casing for a specific symbol kind.
    #[must_use]
    pub fn with(mut self, kind: SymbolKind, style: CaseStyle) -> Self {
        self.overrides.insert(kind, style);
        self
    }

    /// Register an additional initialism (stored lower-cased for lookup).
    #[must_use]
    pub fn with_acronym(mut self, acronym: &str) -> Self {
        self.acronyms.insert(acronym.to_lowercase());
        self
    }

    fn style_for(&self, kind: SymbolKind) -> CaseStyle {
        self.overrides.get(&kind).copied().unwrap_or(self.default)
    }
}

/// Transform a snake_case canonical name into the casing for `kind`.
///
/// A `rename` override (the `@rename(lang)` axis) bypasses casing entirely: it
/// is the literal identifier, cosmetic only, and never affects any wire key.
pub fn transform(
    canonical: &str,
    kind: SymbolKind,
    config: &CasingConfig,
    rename: Option<&str>,
) -> String {
    if let Some(literal) = rename {
        return literal.to_string();
    }
    let words: Vec<&str> = canonical.split('_').filter(|w| !w.is_empty()).collect();
    render(config.style_for(kind), &words, &config.acronyms)
}

fn render(style: CaseStyle, words: &[&str], acronyms: &BTreeSet<String>) -> String {
    match style {
        CaseStyle::Snake => words
            .iter()
            .map(|w| w.to_lowercase())
            .collect::<Vec<_>>()
            .join("_"),
        CaseStyle::ScreamingSnake => words
            .iter()
            .map(|w| w.to_uppercase())
            .collect::<Vec<_>>()
            .join("_"),
        CaseStyle::Pascal => words.iter().map(|w| segment(w, acronyms)).collect(),
        // The leading word of a camelCase identifier is lower-cased wholesale
        // (so `id` stays `id`); only later words get acronym/title treatment.
        CaseStyle::Camel => words
            .iter()
            .enumerate()
            .map(|(i, w)| {
                if i == 0 {
                    w.to_lowercase()
                } else {
                    segment(w, acronyms)
                }
            })
            .collect(),
    }
}

fn segment(word: &str, acronyms: &BTreeSet<String>) -> String {
    if acronyms.contains(&word.to_lowercase()) {
        word.to_uppercase()
    } else {
        capitalize(word)
    }
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(default: CaseStyle) -> CasingConfig {
        CasingConfig::new(default)
    }

    #[test]
    fn casing_table() {
        // (canonical, style, expected)
        let cases = [
            ("payment_method", CaseStyle::Pascal, "PaymentMethod"),
            ("payment_method", CaseStyle::Camel, "paymentMethod"),
            ("payment_method", CaseStyle::Snake, "payment_method"),
            (
                "payment_method",
                CaseStyle::ScreamingSnake,
                "PAYMENT_METHOD",
            ),
            // Initialisms are re-upcased in word positions.
            ("http_url_id", CaseStyle::Pascal, "HTTPURLID"),
            ("http_url_id", CaseStyle::Camel, "httpURLID"),
            ("http_url_id", CaseStyle::Snake, "http_url_id"),
            ("http_url_id", CaseStyle::ScreamingSnake, "HTTP_URL_ID"),
            // A leading acronym in camelCase stays lower-cased wholesale.
            ("id", CaseStyle::Camel, "id"),
            ("id", CaseStyle::Pascal, "ID"),
            // A single ordinary word.
            ("charge", CaseStyle::Pascal, "Charge"),
            ("charge", CaseStyle::Camel, "charge"),
        ];
        for (canonical, style, expected) in cases {
            let got = transform(canonical, SymbolKind::Type, &config(style), None);
            assert_eq!(got, expected, "{canonical} as {style:?}");
        }
    }

    #[test]
    fn rename_bypasses_the_transform() {
        // The literal is used verbatim, regardless of canonical or style.
        let got = transform(
            "payment_method",
            SymbolKind::Field,
            &config(CaseStyle::Camel),
            Some("PaymentMethodV2"),
        );
        assert_eq!(got, "PaymentMethodV2");
    }

    #[test]
    fn per_kind_override_beats_the_default() {
        // Default camel for fields, but types render Pascal.
        let cfg = config(CaseStyle::Camel).with(SymbolKind::Type, CaseStyle::Pascal);
        assert_eq!(
            transform("payment_method", SymbolKind::Type, &cfg, None),
            "PaymentMethod"
        );
        assert_eq!(
            transform("payment_method", SymbolKind::Field, &cfg, None),
            "paymentMethod"
        );
    }

    #[test]
    fn a_custom_acronym_is_recognized() {
        let cfg = config(CaseStyle::Pascal).with_acronym("acme");
        assert_eq!(transform("acme_id", SymbolKind::Type, &cfg, None), "ACMEID");
    }

    #[test]
    fn empty_and_blank_canonical_render_empty() {
        assert_eq!(
            transform("", SymbolKind::Type, &config(CaseStyle::Pascal), None),
            ""
        );
        // Stray underscores produce no words.
        assert_eq!(
            transform("__", SymbolKind::Field, &config(CaseStyle::Camel), None),
            ""
        );
    }

    #[test]
    fn capitalize_handles_an_empty_word() {
        // Unreachable through `transform` (empty words are filtered) but kept
        // total; assert it directly so the branch is covered and stays correct.
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("aBC"), "Abc");
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    // snake_case canonical names over a small word pool (including acronyms and
    // empty segments) so word-boundary handling is exercised.
    fn canonical() -> impl Strategy<Value = String> {
        proptest::collection::vec(
            proptest::sample::select(vec![
                "http", "url", "id", "payment", "method", "v2", "x", "",
            ]),
            0..4,
        )
        .prop_map(|words| words.join("_"))
    }

    fn style() -> impl Strategy<Value = CaseStyle> {
        prop_oneof![
            Just(CaseStyle::Pascal),
            Just(CaseStyle::Camel),
            Just(CaseStyle::Snake),
            Just(CaseStyle::ScreamingSnake),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// The `@rename` literal is emitted verbatim regardless of canonical name,
        /// kind, or style.
        #[test]
        fn rename_always_wins(
            canonical in canonical(),
            s in style(),
            lit in "[A-Za-z][A-Za-z0-9]{0,8}",
        ) {
            let cfg = CasingConfig::new(s);
            prop_assert_eq!(
                transform(&canonical, SymbolKind::Field, &cfg, Some(&lit)),
                lit
            );
        }

        /// Word-joined styles fuse the snake_case words: no underscore survives.
        #[test]
        fn pascal_and_camel_drop_underscores(canonical in canonical()) {
            let pascal =
                transform(&canonical, SymbolKind::Type, &CasingConfig::new(CaseStyle::Pascal), None);
            let camel =
                transform(&canonical, SymbolKind::Field, &CasingConfig::new(CaseStyle::Camel), None);
            prop_assert!(!pascal.contains('_'));
            prop_assert!(!camel.contains('_'));
        }

        /// Snake casing is a fixed point: re-casing snake output is a no-op, since
        /// the canonical form is already snake_case.
        #[test]
        fn snake_is_a_fixed_point(canonical in canonical()) {
            let cfg = CasingConfig::new(CaseStyle::Snake);
            let once = transform(&canonical, SymbolKind::Field, &cfg, None);
            let twice = transform(&once, SymbolKind::Field, &cfg, None);
            prop_assert_eq!(once, twice);
        }
    }
}

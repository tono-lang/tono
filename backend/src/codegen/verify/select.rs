//! Which (ext, language) pairs a check covers.
//!
//! The unit of work is the pair: one probe per ext per language, and
//! nothing about a pair's probe depends on the others running. Every pair
//! by default; a caller re-checking one edited block (an editor, on save)
//! names that pair alone and gets for it exactly what the whole check would
//! have said.

use crate::config::normalize_ext_lang;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Selection(Vec<(String, String)>);

impl Selection {
    /// Every pair the model declares.
    pub fn all() -> Self {
        Self::default()
    }

    /// Only the named pairs; either language spelling is accepted.
    pub fn pairs(pairs: &[(String, String)]) -> Self {
        Self(
            pairs
                .iter()
                .map(|(ext, lang)| (ext.clone(), normalize_ext_lang(lang).to_string()))
                .collect(),
        )
    }

    pub fn allows(&self, ext: &str, lang: &str) -> bool {
        self.0.is_empty()
            || self
                .0
                .iter()
                .any(|(e, l)| e == ext && l == normalize_ext_lang(lang))
    }

    /// Whether any selected pair is in `lang` (a language-level note is
    /// only worth printing when that language is being checked).
    pub fn allows_lang(&self, lang: &str) -> bool {
        self.0.is_empty() || self.0.iter().any(|(_, l)| l == normalize_ext_lang(lang))
    }
}

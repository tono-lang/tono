//! What a generated file is, and which language produced it.
//!
//! Separate from the pipeline that builds them so the type every layer names
//! (targets, layout, the CLI) does not pull the whole engine in.

use std::path::PathBuf;

/// A language the generator can emit.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TargetKind {
    Rust,
    Go,
    TypeScript,
}

impl TargetKind {
    /// Parse a target name as accepted on the command line (`ts` aliases
    /// `typescript`).
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "rust" => Some(Self::Rust),
            "go" => Some(Self::Go),
            "typescript" | "ts" => Some(Self::TypeScript),
            _ => None,
        }
    }

    /// The conventional output subdirectory for this target.
    pub fn dir(self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Go => "go",
            Self::TypeScript => "typescript",
        }
    }

    /// The source-file extension for this target.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Rust => "rs",
            Self::Go => "go",
            Self::TypeScript => "ts",
        }
    }

    /// The `ext` binding keys that reach this target, in preference order. The
    /// per-target emitters read the same list, so a language spelled one way in
    /// the manifest and another in a binding still resolves.
    pub fn binding_langs(self) -> &'static [&'static str] {
        match self {
            Self::Rust => &["rust"],
            Self::Go => &["go"],
            Self::TypeScript => &["ts", "typescript"],
        }
    }
}

/// A declaration's byte range in a file's rough text, keyed by the symbols the
/// declaration declares. Offsets index the text exactly as [`super::pipeline::generate`]
/// returns it (the engine's own layout); running a real formatter over the text
/// invalidates them, which is why the CLI writer does not carry them along.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclSpan {
    pub symbols: Vec<String>,
    pub start: usize,
    pub end: usize,
}

/// A generated source file: which target produced it (so a caller knows which
/// formatter to run), its path relative to the output root, its text, and where
/// each declaration landed in that text (for tooling that maps IR declarations
/// to emitted code; empty for layout-only files like module trees and barrels).
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedFile {
    pub target: TargetKind,
    pub path: PathBuf,
    pub text: String,
    pub decl_spans: Vec<DeclSpan>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_kind_parses_names_and_aliases() {
        assert_eq!(TargetKind::parse("rust"), Some(TargetKind::Rust));
        assert_eq!(TargetKind::parse("go"), Some(TargetKind::Go));
        assert_eq!(
            TargetKind::parse("typescript"),
            Some(TargetKind::TypeScript)
        );
        assert_eq!(TargetKind::parse("ts"), Some(TargetKind::TypeScript));
        assert_eq!(TargetKind::parse("java"), None);
    }

    #[test]
    fn target_kind_dirs_and_extensions() {
        for (target, dir, ext) in [
            (TargetKind::Rust, "rust", "rs"),
            (TargetKind::Go, "go", "go"),
            (TargetKind::TypeScript, "typescript", "ts"),
        ] {
            assert_eq!(target.dir(), dir);
            assert_eq!(target.extension(), ext);
        }
    }
}

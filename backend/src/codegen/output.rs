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

    /// Whether this target's codegen can emit an extern-call field source
    /// (`field: T = ns.fn(args)`). This is the one place a target declares
    /// that capability, flipped by that target's own emission work; every
    /// other layer asks here instead of naming a target, so landing the
    /// next target changes this arm and nothing else.
    pub fn emits_ext_calls(self) -> bool {
        match self {
            Self::Go | Self::TypeScript | Self::Rust => true,
        }
    }

    /// Whether this target's codegen can emit a foreign opaque-handle type
    /// on the entry surface (a field whose target is a `type` declared
    /// inside an `ext` block). Separate from [`Self::emits_ext_calls`]: a
    /// target can emit the call that returns a handle before it can also
    /// spell the handle's own field type, and conflating the two would let
    /// a handle-typed field through validation to an emitter with no case
    /// for it.
    pub fn emits_ext_handle_types(self) -> bool {
        match self {
            Self::Go | Self::TypeScript | Self::Rust => true,
        }
    }

    /// Whether this target's codegen can emit an operation's own
    /// `impl .field.method(args)` body: a call into a foreign handle's
    /// method, standing in for the wire protocol. Separate from
    /// [`Self::emits_ext_handle_types`]: a target can spell a handle's field
    /// type before it can also render a call through it, and conflating the
    /// two would let an operation whose only implementation is this call
    /// through validation to an emitter with no case for it.
    pub fn emits_ext_handle_calls(self) -> bool {
        match self {
            Self::Go | Self::TypeScript | Self::Rust => true,
        }
    }

    /// Whether this target's codegen can render a cross-extern call
    /// (`ns.fn(..)`, resolved against a declared `extern`) standing as a
    /// ctor field's own value inside a `call:` line, e.g.
    /// `call: "Load"(opts { token: auth.sign() })`. Separate from
    /// [`Self::emits_ext_calls`]: a target can emit the ordinary call
    /// before it can also render one of *its own* arguments as another
    /// declared call. Only Rust does today.
    pub fn emits_nested_extern_call_args(self) -> bool {
        match self {
            Self::Rust => true,
            Self::Go | Self::TypeScript => false,
        }
    }
}

/// A generated source file: which target produced it (so a caller knows which
/// formatter to run), its path relative to the output root, and its text.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratedFile {
    pub target: TargetKind,
    pub path: PathBuf,
    pub text: String,
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

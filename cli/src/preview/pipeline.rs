//! The preview pipeline: a `.tono` path in, a rendered-and-checked snapshot out.
//!
//! One pass runs the whole chain the preview shows: read the source, compile it to
//! IR through the frontend, generate the SDK for the chosen target, then run that
//! target's native checker over the output. The result is a [`Snapshot`] the UI
//! renders directly. The chain short-circuits at the first thing that goes wrong
//! (an unreadable file, a source the frontend rejects), so the UI always has a
//! single clear verdict to show.

use std::path::Path;

use tono_backend::codegen::{
    check, check_layout, generate, CheckOptions, CheckOutcome, CodegenConfig, Formatter,
    GeneratedFile, TargetKind,
};
use tono_backend::ir::decode_model;

use crate::frontend::{Frontend, FrontendError};

/// The outcome of one preview pass: the source shown on the left, the generated
/// code shown on the right, and the verdict shown in the status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub source: String,
    pub target: TargetKind,
    /// The generated code for the target, empty when the pipeline stopped before
    /// generating (an unreadable source, or one the frontend rejected).
    pub generated: String,
    pub verdict: Verdict,
}

/// The single headline the preview reports, from furthest-upstream failure to a
/// clean compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The source file could not be read.
    SourceUnreadable(String),
    /// The frontend binary is not installed.
    FrontendUnavailable(String),
    /// The frontend rejected the source; carries its diagnostics.
    SourceError(String),
    /// The IR could not be decoded (a frontend/backend version skew).
    IrError(String),
    /// The generated SDK compiled under the target's toolchain.
    Compiles,
    /// The toolchain ran and rejected the generated SDK; carries its diagnostics.
    DoesNotCompile(String),
    /// The target's toolchain is not installed, so no verdict could be reached.
    ToolchainMissing(String),
    /// The checker could not be set up (its scratch project could not be written).
    CheckError(String),
    /// The generator rejected the model (e.g. the bespoke conformance gate);
    /// carries its message.
    GenerateRejected(String),
}

impl Verdict {
    /// A one-line summary for the status area. Compile diagnostics are long and
    /// shown in their own region, so the headline names the outcome and leaves the
    /// detail to the caller.
    pub fn headline(&self) -> String {
        match self {
            Verdict::SourceUnreadable(e) => format!("cannot read source: {e}"),
            Verdict::FrontendUnavailable(program) => {
                format!("frontend '{program}' not found (set TONO_FRONTEND)")
            }
            Verdict::SourceError(_) => "source does not compile (see diagnostics)".into(),
            Verdict::IrError(e) => format!("IR error: {e}"),
            Verdict::Compiles => "compiles".into(),
            Verdict::DoesNotCompile(_) => "does not compile (see diagnostics)".into(),
            Verdict::ToolchainMissing(program) => {
                format!("toolchain '{program}' not installed; cannot check")
            }
            Verdict::CheckError(e) => format!("could not run the checker: {e}"),
            Verdict::GenerateRejected(_) => "generation rejected (see diagnostics)".into(),
        }
    }

    /// The diagnostics behind the headline, when there are any to show (a rejected
    /// source or a rejected build). The other verdicts are fully said by the
    /// headline, so they carry no detail.
    pub fn detail(&self) -> Option<&str> {
        match self {
            Verdict::SourceError(d) | Verdict::DoesNotCompile(d) | Verdict::GenerateRejected(d) => {
                Some(d)
            }
            _ => None,
        }
    }
}

/// Run the whole chain for `source_path` and `target`. `scratch` is the throwaway
/// build directory (reused across passes so the toolchain keeps its incremental
/// cache); `options` carries the TypeScript runtime mapping. Every problem, from an
/// unreadable file to a scaffolding failure, is a [`Verdict`], so a pass always
/// yields a [`Snapshot`] the UI can render.
pub fn run(
    frontend: &Frontend,
    source_path: &Path,
    target: TargetKind,
    scratch: &Path,
    options: &CheckOptions,
) -> Snapshot {
    let source = match std::fs::read_to_string(source_path) {
        Ok(text) => text,
        Err(e) => {
            return stopped(
                String::new(),
                target,
                Verdict::SourceUnreadable(e.to_string()),
            )
        }
    };

    let ir = match frontend.compile(source_path) {
        Ok(ir) => ir,
        Err(FrontendError::Unavailable { program }) => {
            return stopped(source, target, Verdict::FrontendUnavailable(program))
        }
        Err(FrontendError::Diagnostics(diags)) => {
            return stopped(source, target, Verdict::SourceError(diags))
        }
    };

    let model = match decode_model(&ir) {
        Ok(model) => model,
        Err(e) => return stopped(source, target, Verdict::IrError(e)),
    };

    // The scaffold the checker builds declares a fixed module path, and Go
    // spells its cross-package imports with it, so generation is given the same
    // one rather than left to fail the layout gate.
    let config = CodegenConfig {
        go_module: Some(tono_backend::codegen::check::GO_SCAFFOLD_MODULE.into()),
        ..CodegenConfig::default()
    };
    // The Go layout gate runs first: its message names the module problem, where
    // the compiler would only show the broken import it causes.
    if let Err(e) = check_layout(&model, &[target], &config) {
        return stopped(source, target, Verdict::GenerateRejected(e));
    }
    let mut files = match generate(&model, &[target], &config) {
        Ok(files) => files,
        Err(e) => return stopped(source, target, Verdict::GenerateRejected(e)),
    };
    // Formatted like `gen` would write it: the pane shows what a consumer would
    // read (a missing formatter degrades to the rough text), and the checker
    // compiles the same bytes the pane shows.
    for file in &mut files {
        file.text = Formatter::for_output(file.target, &file.path)
            .run(&file.text)
            .text;
    }
    let generated = assemble(&files);
    let verdict = match check(target, &files, scratch, options) {
        Ok(outcome) => verdict_of(outcome),
        Err(e) => Verdict::CheckError(e.to_string()),
    };
    Snapshot {
        source,
        target,
        generated,
        verdict,
    }
}

/// A snapshot that stopped before generating: no generated code, just the verdict.
fn stopped(source: String, target: TargetKind, verdict: Verdict) -> Snapshot {
    Snapshot {
        source,
        target,
        generated: String::new(),
        verdict,
    }
}

/// Concatenate the generated files into one pane, each under a `// <path>` header
/// so a multi-file target (types plus serde) reads as a labeled sequence rather
/// than a wall of code. Single-file targets get a single header, which is cheap
/// and keeps the format uniform.
fn assemble(files: &[GeneratedFile]) -> String {
    files
        .iter()
        .map(|f| format!("// {}\n\n{}", f.path.display(), f.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Translate the backend's compile verdict into the preview's.
fn verdict_of(outcome: CheckOutcome) -> Verdict {
    match outcome {
        CheckOutcome::Compiles => Verdict::Compiles,
        CheckOutcome::Failed { diagnostics } => Verdict::DoesNotCompile(diagnostics),
        CheckOutcome::ToolchainMissing { program } => Verdict::ToolchainMissing(program),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn file(path: &str, text: &str) -> GeneratedFile {
        GeneratedFile {
            target: TargetKind::Rust,
            path: PathBuf::from(path),
            text: text.to_string(),
            decl_spans: Vec::new(),
        }
    }

    #[test]
    fn assemble_labels_each_file_with_its_path() {
        let files = vec![
            file("rust/demo.rs", "pub struct Charge;\n"),
            file("rust/demo_serde.rs", "// serde\n"),
        ];
        let out = assemble(&files);
        assert!(out.contains("// rust/demo.rs\n\npub struct Charge;"));
        assert!(out.contains("// rust/demo_serde.rs\n\n// serde"));
    }

    #[test]
    fn verdict_maps_each_check_outcome() {
        assert_eq!(verdict_of(CheckOutcome::Compiles), Verdict::Compiles);
        assert_eq!(
            verdict_of(CheckOutcome::Failed {
                diagnostics: "boom".into()
            }),
            Verdict::DoesNotCompile("boom".into())
        );
        assert_eq!(
            verdict_of(CheckOutcome::ToolchainMissing {
                program: "cargo".into()
            }),
            Verdict::ToolchainMissing("cargo".into())
        );
    }

    #[test]
    fn an_unreadable_source_stops_before_the_frontend() {
        let frontend = Frontend::from_env();
        let missing = std::env::temp_dir().join("tono-preview-does-not-exist.tono");
        let _ = std::fs::remove_file(&missing);
        let snapshot = run(
            &frontend,
            &missing,
            TargetKind::Rust,
            &std::env::temp_dir(),
            &CheckOptions::default(),
        );
        assert!(matches!(snapshot.verdict, Verdict::SourceUnreadable(_)));
        assert!(snapshot.generated.is_empty());
    }
}

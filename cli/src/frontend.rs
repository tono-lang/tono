//! The bridge to the OCaml frontend: turn a `.tono` file into IR JSON by running
//! `tono-frontend compile` as a subprocess.
//!
//! The frontend is the single source of truth for the IR, so the preview reuses
//! it rather than reimplementing the parse and typecheck. Like the formatter and
//! the checker, a missing binary is a distinct, surfaced outcome rather than a
//! hard failure.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Resolve the frontend binary. First hit wins: `TONO_FRONTEND`, a sibling of
/// the running `tono` (an installed pair), the dune build tree next to this
/// crate (a dev checkout works with no setup), then bare `tono-frontend` on
/// `PATH`.
pub fn resolve_program() -> String {
    if let Some(p) = std::env::var_os("TONO_FRONTEND") {
        return p.to_string_lossy().into_owned();
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("tono-frontend"));
            candidates.push(dir.join("tono_frontend"));
            candidates.push(dir.join("tono_frontend.exe"));
        }
    }
    if let Some(root) = Path::new(env!("CARGO_MANIFEST_DIR")).parent() {
        candidates.push(root.join("_build/default/frontend/bin/tono_frontend.exe"));
    }
    candidates
        .into_iter()
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tono-frontend".into())
}

/// Why a compile did not yield IR. A source that does not typecheck and a missing
/// frontend are different problems: the first is the user's `.tono` to fix, the
/// second is an environment gap, so the preview presents them differently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrontendError {
    /// The frontend binary could not be spawned.
    Unavailable { program: String },
    /// The frontend ran and rejected the source; carries its diagnostics.
    Diagnostics(String),
}

/// A handle to the `tono-frontend` binary.
pub struct Frontend {
    program: String,
}

impl Frontend {
    /// The frontend as [`resolve_program`] finds it.
    pub fn from_env() -> Self {
        Self {
            program: resolve_program(),
        }
    }

    /// Compile `path` to canonical IR JSON. The module name is left to the
    /// frontend, which derives it from the file stem.
    pub fn compile(&self, path: &Path) -> Result<String, FrontendError> {
        self.invoke("compile", path)
    }

    /// Compile every `.tono` under `root` into one multi-module IR model: a
    /// whole project in a single pass, which is what generating an SDK from
    /// sources needs. A root holding no sources is a diagnostic, not an empty
    /// model, so the caller reports it rather than emitting nothing.
    pub fn compile_dir(&self, root: &Path) -> Result<String, FrontendError> {
        self.invoke("compile-dir", root)
    }

    /// The source span of every foreign binding in `path`, one JSON object
    /// per line (see `tono_backend::codegen::verify::parse_sites`).
    pub fn ext_bindings(&self, path: &Path) -> Result<String, FrontendError> {
        self.invoke("ext-bindings", path)
    }

    fn invoke(&self, sub: &str, path: &Path) -> Result<String, FrontendError> {
        match Command::new(&self.program).arg(sub).arg(path).output() {
            Ok(out) => interpret(
                out.status.success(),
                &String::from_utf8_lossy(&out.stdout),
                &String::from_utf8_lossy(&out.stderr),
            ),
            Err(_) => Err(FrontendError::Unavailable {
                program: self.program.clone(),
            }),
        }
    }
}

/// Map the frontend's exit and streams to a result: on success the IR JSON is on
/// stdout; on failure the diagnostics are on stderr (falling back to stdout if the
/// frontend wrote there instead, so an error is never reported empty).
fn interpret(success: bool, stdout: &str, stderr: &str) -> Result<String, FrontendError> {
    if success {
        Ok(stdout.to_string())
    } else {
        let message = if stderr.trim().is_empty() {
            stdout
        } else {
            stderr
        };
        Err(FrontendError::Diagnostics(message.trim().to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_returns_stdout_as_ir() {
        assert_eq!(
            interpret(true, "{\"tono_ir_version\":5}\n", ""),
            Ok("{\"tono_ir_version\":5}\n".to_string())
        );
    }

    #[test]
    fn failure_returns_stderr_diagnostics_trimmed() {
        assert_eq!(
            interpret(false, "", "line 1: unexpected token\n"),
            Err(FrontendError::Diagnostics(
                "line 1: unexpected token".into()
            ))
        );
    }

    #[test]
    fn failure_falls_back_to_stdout_when_stderr_is_empty() {
        // A frontend that reported on the wrong stream still surfaces a message
        // rather than an empty error.
        assert_eq!(
            interpret(false, "some error\n", "   \n"),
            Err(FrontendError::Diagnostics("some error".into()))
        );
    }

    #[test]
    fn a_missing_binary_is_unavailable() {
        let frontend = Frontend {
            program: "tono-no-such-frontend-xyz".into(),
        };
        assert_eq!(
            frontend.compile(Path::new("whatever.tono")),
            Err(FrontendError::Unavailable {
                program: "tono-no-such-frontend-xyz".into()
            })
        );
    }
}

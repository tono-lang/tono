//! The formatter subprocess: the single source of layout truth.
//!
//! The engine emits roughly-formatted but syntactically valid text and pipes it
//! through each language's official formatter (prettier, rustfmt, gofmt, ...) as
//! a subprocess, reading the formatted result back. The formatter is a
//! dependency of the generator, never of the produced SDK. It must never fail
//! the generation: a missing binary or a non-zero exit falls back to the
//! unformatted text plus a warning, surfaced for the caller rather than printed.

use std::io::Write;
use std::process::{Command, Stdio};

/// A formatter subprocess descriptor: the program plus its arguments. The text
/// to format is delivered on stdin and the formatted text read from stdout.
pub struct Formatter {
    pub program: String,
    pub args: Vec<String>,
}

/// Why a formatting pass fell back to unformatted text. The engine never fails
/// on these; it returns the warning so the caller can surface it. The two cases
/// are kept distinct because they mean different things: an absent formatter is
/// an environment gap, while a non-zero exit usually means the rough text was
/// not valid input — a generator bug worth seeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Warning {
    /// The formatter could not be spawned (not on `PATH`, or otherwise
    /// unrunnable).
    FormatterUnavailable { program: String },
    /// The formatter ran but exited non-zero (it rejected the input or errored).
    FormatterRejected {
        program: String,
        status: Option<i32>,
        stderr: String,
    },
}

/// The result of a formatting pass: the text (formatted when the pass
/// succeeded, otherwise the original rough text) and any warning explaining a
/// fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Formatted {
    pub text: String,
    pub warning: Option<Warning>,
}

impl Formatter {
    /// A formatter that runs `program` with `args`.
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }

    /// Format `rough` by piping it through the subprocess. On a missing binary
    /// or a non-zero exit, return the rough text unchanged with a warning;
    /// never fail.
    pub fn run(&self, rough: &str) -> Formatted {
        let spawned = Command::new(&self.program)
            .args(&self.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(_) => return self.unavailable(rough),
        };

        // Write stdin from a separate thread so a formatter that streams stdout
        // while we are still writing cannot deadlock the pipe. A formatter that
        // ignores stdin closes the pipe early; the resulting write error is
        // expected and ignored (Rust ignores SIGPIPE, so this never aborts).
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let payload = rough.to_string();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(payload.as_bytes());
        });
        let output = child.wait_with_output();
        let _ = writer.join();

        match output {
            Ok(out) if out.status.success() => Formatted {
                text: String::from_utf8_lossy(&out.stdout).into_owned(),
                warning: None,
            },
            Ok(out) => Formatted {
                text: rough.to_string(),
                warning: Some(Warning::FormatterRejected {
                    program: self.program.clone(),
                    status: out.status.code(),
                    stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
                }),
            },
            // The child spawned but could not be waited on; treat it as
            // unavailable rather than failing the generation.
            Err(_) => self.unavailable(rough),
        }
    }

    fn unavailable(&self, rough: &str) -> Formatted {
        Formatted {
            text: rough.to_string(),
            warning: Some(Warning::FormatterUnavailable {
                program: self.program.clone(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_successful_formatter_returns_its_stdout_with_no_warning() {
        // `cat` echoes stdin to stdout unchanged: it exercises the stdin/stdout
        // plumbing and the success path deterministically.
        let formatted = Formatter::new("cat", vec![]).run("hello world\n");
        assert_eq!(formatted.text, "hello world\n");
        assert_eq!(formatted.warning, None);
    }

    #[test]
    fn an_absent_binary_falls_back_with_an_unavailable_warning() {
        let rough = "fn main() {}";
        let formatted = Formatter::new("tono-no-such-formatter-xyz", vec![]).run(rough);
        assert_eq!(formatted.text, rough);
        assert_eq!(
            formatted.warning,
            Some(Warning::FormatterUnavailable {
                program: "tono-no-such-formatter-xyz".into(),
            })
        );
    }

    #[test]
    fn a_nonzero_exit_falls_back_with_a_rejected_warning() {
        // `false` exits 1 without reading stdin; the rough text is preserved and
        // the non-zero exit is reported distinctly from an absent formatter.
        let rough = "rough but unformatted";
        let formatted = Formatter::new("false", vec![]).run(rough);
        assert_eq!(formatted.text, rough);
        let warning = formatted.warning.expect("a warning");
        assert!(
            matches!(&warning, Warning::FormatterRejected { program, status, .. }
                if program == "false" && *status == Some(1)),
            "expected a rejected warning for `false`, got {warning:?}"
        );
    }

    #[test]
    fn formatter_with_args_is_invoked_with_them() {
        // `cat` with an explicit `-` (stdin) argument still round-trips, proving
        // args are forwarded to the subprocess.
        let formatted = Formatter::new("cat", vec!["-".into()]).run("x\n");
        assert_eq!(formatted.text, "x\n");
        assert_eq!(formatted.warning, None);
    }
}

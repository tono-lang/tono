//! Checking `ext` declarations against the real foreign library, reported on
//! the `.tono` that declared them.
//!
//! A foreign spelling (`#(...)`) is text the frontend cannot validate: the
//! target compiler is what catches a wrong one, in a generated file on a
//! line the author never wrote. This module gives that finding back to the
//! source. For every binding it writes one probe line that crosses the
//! boundary exactly the way the emitter will (a parameter of the type the
//! binding says crosses, the context where it is declared, the results the
//! target's convention assigns) and has the target's own toolchain check
//! the probe against the library installed in the consumer tree. A line the
//! compiler rejects maps back to the binding's span in the `.tono`.
//!
//! Two rules shape everything here. The probe checks what was declared and
//! never fills in what was not: a missing annotation is left missing and
//! the default mapping crosses, as it does in the generated SDK. And a
//! library whose types cannot be read (no module to resolve, no `.d.ts`) is
//! not a failure but a line in the report: the declaration stands
//! unchecked and the report says so.
//!
//! The per-target probe writers live next to the emitters they mirror
//! (`targets::go::entry::verify`, `targets::typescript::entry::verify`).
//! Rust bindings are reported as unchecked: reading a crate's signatures
//! needs rustdoc's JSON output, which only a nightly toolchain produces.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::codegen::targets::{go, typescript};
use crate::config::normalize_ext_lang;
use crate::ir::{ExtLib, Model, Module};

/// The stable code every foreign-signature finding carries.
pub const FINDING_CODE: &str = "FX0001";

/// What a binding site is, as the frontend's `ext-bindings` listing names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SiteKind {
    /// The per-language module path.
    Path,
    /// An opaque handle's storage block.
    Handle,
    /// A foreign form's language block.
    Struct,
    /// A free op's `call:` line.
    Op,
    /// A handle method's `call:` line (`owner` names the handle).
    Method,
}

/// One binding's source location, keyed the way the IR names it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Site {
    pub ext: String,
    pub lang: String,
    pub kind: SiteKind,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Rendered the way the frontend prints a diagnostic span (`12:5-40`).
    pub span: String,
}

/// Parse the frontend's `ext-bindings` output: one JSON object per line.
pub fn parse_sites(text: &str) -> Result<Vec<Site>, String> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).map_err(|e| format!("ext-bindings line {l:?}: {e}")))
        .collect()
}

/// The key a probe line carries to find its site: everything but the ext,
/// which is the probe's own.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SiteKey {
    pub kind: SiteKind,
    pub owner: Option<String>,
    pub name: Option<String>,
}

impl SiteKey {
    pub fn path() -> Self {
        Self {
            kind: SiteKind::Path,
            owner: None,
            name: None,
        }
    }
    pub fn handle(name: &str) -> Self {
        Self {
            kind: SiteKind::Handle,
            owner: None,
            name: Some(name.to_string()),
        }
    }
    pub fn form(name: &str) -> Self {
        Self {
            kind: SiteKind::Struct,
            owner: None,
            name: Some(name.to_string()),
        }
    }
    pub fn op(owner: Option<&str>, name: &str) -> Self {
        Self {
            kind: if owner.is_some() {
                SiteKind::Method
            } else {
                SiteKind::Op
            },
            owner: owner.map(str::to_string),
            name: Some(name.to_string()),
        }
    }

    /// How a report names the binding: `op from_series`, `method
    /// calculator.compute`, `handle calculator`, `struct formula_options`.
    pub fn label(&self) -> String {
        match (self.kind, &self.owner, &self.name) {
            (SiteKind::Path, _, _) => "module path".to_string(),
            (SiteKind::Handle, _, Some(n)) => format!("handle {n}"),
            (SiteKind::Struct, _, Some(n)) => format!("struct {n}"),
            (SiteKind::Method, Some(o), Some(n)) => format!("method {o}.{n}"),
            (_, _, Some(n)) => format!("op {n}"),
            (kind, _, None) => format!("{kind:?}").to_lowercase(),
        }
    }
}

/// One target's probe for one ext: the source to compile, which line
/// stands for which binding, and the bindings the probe could not express
/// (each with why), which the report lists as unchecked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Probe {
    pub source: String,
    pub lines: BTreeMap<usize, SiteKey>,
    pub skipped: Vec<String>,
}

impl Probe {
    /// Append one probe line standing for `key`.
    pub fn push(&mut self, key: &SiteKey, line: &str) {
        self.source.push_str(line);
        self.source.push('\n');
        let number = self.source.lines().count();
        self.lines.insert(number, key.clone());
    }

    /// Append a line that stands for no binding (a package clause, a blank).
    pub fn push_plain(&mut self, line: &str) {
        self.source.push_str(line);
        self.source.push('\n');
    }

    pub fn skip(&mut self, key: &SiteKey, reason: &str) {
        self.skipped.push(format!("{}: {reason}", key.label()));
    }
}

/// A divergence between a declaration and the library, at its `.tono` span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub span: String,
    pub message: String,
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: error: {FINDING_CODE}: {}", self.span, self.message)
    }
}

/// What the check produced: findings to fix, and what it could not check.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// One line per thing left unchecked, each saying why.
    pub unchecked: Vec<String>,
    /// One line per (ext, language) the toolchain accepted.
    pub checked: Vec<String>,
}

/// Where each target's library is resolved from: a directory inside the
/// consumer tree for that language (a Go module that requires the library,
/// a tree whose `node_modules` holds the package). `None` leaves that
/// language unchecked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibRoots {
    pub go: Option<PathBuf>,
    pub ts: Option<PathBuf>,
}

/// One error the toolchain reported, located on a probe line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompilerError {
    /// `None` when the message carried no position in the probe file.
    pub line: Option<usize>,
    pub message: String,
}

/// How a probe run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    Passed,
    Failed(Vec<CompilerError>),
    ToolchainMissing { program: String },
}

/// A ready-to-run toolchain invocation on a probe written to disk.
pub struct ProbeRun {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// The probe file's path as the toolchain will print it, to recognise
    /// its own lines in the output.
    pub file_label: String,
    pub parse: fn(&str, &str) -> Vec<CompilerError>,
}

/// Run one probe and map its output.
pub fn run_probe(run: &ProbeRun) -> RunOutcome {
    let output = match Command::new(&run.program)
        .args(&run.args)
        .current_dir(&run.cwd)
        .output()
    {
        Ok(o) => o,
        Err(_) => {
            return RunOutcome::ToolchainMissing {
                program: run.program.display().to_string(),
            }
        }
    };
    if output.status.success() {
        return RunOutcome::Passed;
    }
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut errors = (run.parse)(&text, &run.file_label);
    if errors.is_empty() {
        errors.push(CompilerError {
            line: None,
            message: text.trim().to_string(),
        });
    }
    RunOutcome::Failed(errors)
}

/// A scratch directory for the probe, inside the consumer tree so the
/// toolchain resolves the library the way the generated SDK does, and
/// removed when the check is over.
pub struct Scratch {
    pub dir: PathBuf,
}

impl Scratch {
    pub fn create(root: &Path, name: &str) -> std::io::Result<Self> {
        let dir = root.join(format!(".tono-check-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Parse `file:line:col: message` lines (Go), keeping the tab-indented
/// continuation lines the compiler prints under a message.
pub fn parse_go_errors(text: &str, file_label: &str) -> Vec<CompilerError> {
    let mut out: Vec<CompilerError> = Vec::new();
    for raw in text.lines() {
        if raw.starts_with('\t') {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(raw);
            }
            continue;
        }
        let Some(rest) = raw
            .strip_prefix(file_label)
            .and_then(|r| r.strip_prefix(':'))
        else {
            if raw.starts_with("go: ") || raw.starts_with("vet: ") {
                out.push(CompilerError {
                    line: None,
                    message: raw.to_string(),
                });
            }
            continue;
        };
        let mut parts = rest.splitn(3, ':');
        let line = parts.next().and_then(|l| l.trim().parse::<usize>().ok());
        let _col = parts.next();
        let message = parts.next().unwrap_or("").trim().to_string();
        out.push(CompilerError { line, message });
    }
    out
}

/// Parse `file(line,col): error TSxxxx: message` lines (tsc with
/// `--pretty false`), keeping the space-indented continuation lines.
pub fn parse_tsc_errors(text: &str, file_label: &str) -> Vec<CompilerError> {
    let mut out: Vec<CompilerError> = Vec::new();
    for raw in text.lines() {
        if raw.starts_with(' ') {
            if let Some(last) = out.last_mut() {
                last.message.push('\n');
                last.message.push_str(raw);
            }
            continue;
        }
        let Some((location, message)) = raw.split_once("): ") else {
            continue;
        };
        let Some((file, position)) = location.split_once('(') else {
            continue;
        };
        let line = (file == file_label)
            .then(|| {
                position
                    .split(',')
                    .next()
                    .and_then(|l| l.parse::<usize>().ok())
            })
            .flatten();
        out.push(CompilerError {
            line,
            message: message.trim().to_string(),
        });
    }
    out
}

/// Fold one probe run's outcome into the report: each error on a binding
/// line becomes a finding at that binding's span; an error on the import
/// line means the library could not be read and leaves the whole ext
/// unchecked; anything else is reported as the probe failing.
fn fold_outcome(
    report: &mut Report,
    outcome: RunOutcome,
    probe: &Probe,
    sites: &[Site],
    ext: &str,
    lang: &str,
) -> Result<(), String> {
    let tool = match lang {
        "go" => "go build",
        _ => "tsc",
    };
    let site_span = |key: &SiteKey| -> Option<String> {
        sites
            .iter()
            .find(|s| {
                s.ext == ext
                    && normalize_ext_lang(&s.lang) == lang
                    && s.kind == key.kind
                    && s.owner == key.owner
                    && s.name == key.name
            })
            .map(|s| s.span.clone())
    };
    let skipped: Vec<String> = probe
        .skipped
        .iter()
        .map(|s| format!("{lang} {s} in ext {ext}"))
        .collect();
    match outcome {
        RunOutcome::Passed => {
            report.unchecked.extend(skipped);
            report
                .checked
                .push(format!("{lang} bindings of ext {ext} ({tool})"));
        }
        RunOutcome::ToolchainMissing { program } => {
            return Err(format!(
                "checking the {lang} bindings of ext {ext} needs {program}, which is not installed \
                 (the check requires the toolchain of every target it checks)"
            ));
        }
        RunOutcome::Failed(errors) => {
            let mut findings = Vec::new();
            let mut unresolved = false;
            for e in errors {
                match e.line.and_then(|l| probe.lines.get(&l)) {
                    Some(k) if k.kind == SiteKind::Path => {
                        unresolved = true;
                        let reason = e.message.lines().next().unwrap_or_default();
                        let reason = reason.split("; to add it:").next().unwrap_or(reason);
                        report.unchecked.push(format!(
                            "{lang} bindings of ext {ext}: no type source ({reason})"
                        ));
                    }
                    Some(k) => findings.push(Finding {
                        span: site_span(k).unwrap_or_else(|| "0:0-0".to_string()),
                        message: format!(
                            "{lang} binding of {} in ext {ext}: {}",
                            k.label(),
                            e.message
                        ),
                    }),
                    None => report.unchecked.push(format!(
                        "{lang} bindings of ext {ext}: the probe failed outside a binding ({})",
                        e.message.lines().next().unwrap_or_default()
                    )),
                }
            }
            // Once the library itself did not resolve, every other error is
            // a consequence of the missing import, not a finding, and what
            // the probe skipped is moot.
            if !unresolved {
                report.findings.extend(findings);
                report.unchecked.extend(skipped);
            }
        }
    }
    Ok(())
}

fn lang_declared(lib: &ExtLib, lang: &str) -> bool {
    lib.langs
        .iter()
        .any(|p| normalize_ext_lang(&p.lang) == lang)
}

/// Check every ext of `model` whose language has a root, and report.
pub fn verify(model: &Model, sites: &[Site], roots: &LibRoots) -> Result<Report, String> {
    let mut report = Report::default();
    for module in &model.modules {
        for lib in &module.ext_libs {
            verify_lib(&mut report, module, lib, sites, roots)?;
        }
    }
    Ok(report)
}

fn verify_lib(
    report: &mut Report,
    module: &Module,
    lib: &ExtLib,
    sites: &[Site],
    roots: &LibRoots,
) -> Result<(), String> {
    if lang_declared(lib, "rust") {
        report.unchecked.push(format!(
            "rust bindings of ext {}: reading a crate's signatures needs rustdoc JSON, nightly only",
            lib.name
        ));
    }
    if lang_declared(lib, "go") {
        match &roots.go {
            None => report.unchecked.push(format!(
                "go bindings of ext {}: no Go module to resolve the library in (set the go target's out in tono.toml, or pass --lib-root go=<dir>)",
                lib.name
            )),
            Some(root) => {
                let probe = go::entry::verify::probe(module, lib);
                let outcome = go::entry::verify::run(root, &probe).map_err(|e| e.to_string())?;
                fold_outcome(report, outcome, &probe, sites, &lib.name, "go")?;
            }
        }
    }
    if lang_declared(lib, "ts") {
        match &roots.ts {
            None => report.unchecked.push(format!(
                "ts bindings of ext {}: no node_modules tree to resolve the package in (set the typescript target's out in tono.toml, or pass --lib-root ts=<dir>)",
                lib.name
            )),
            Some(root) => {
                let probe = typescript::entry::verify::probe(module, lib);
                let outcome = typescript::entry::verify::run(root, &probe).map_err(|e| e.to_string())?;
                fold_outcome(report, outcome, &probe, sites, &lib.name, "ts")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

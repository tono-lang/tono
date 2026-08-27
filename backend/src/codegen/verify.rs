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
//! A binding may name one of the module's own types (`#(Memo<.reading>)`, a
//! parameter typed `reading`, a struct passed as a class). The probe compiles
//! beside those types: the SDK's type declarations are generated in memory
//! by the same pipeline `tono gen` runs ([`generated_types`]) and written
//! into the scratch directory, and the probe sits where the emitted ext glue
//! would, in the module's own package or directory, naming them exactly as
//! the glue does. This makes the check depend on the emitter for the types
//! it names, which is the intent (the tono side is what the library is
//! confronted with), and it also means a defect in an emitted types file is
//! not a finding here: it surfaces as the probe failing outside a binding,
//! and the target compiler on the generated SDK stays the gate for the
//! emitter itself. When generation refuses the model, the types are not
//! there, and every binding that needs them is listed with that reason.
//!
//! The per-target probe writers live next to the emitters they mirror
//! (`targets::go::entry::verify`, `targets::typescript::entry::verify`).
//! Rust bindings are reported as unchecked: reading a crate's signatures
//! needs rustdoc's JSON output, which only a nightly toolchain produces.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::codegen::casing::CaseStyle;
use crate::codegen::modules::{self, CodegenConfig};
use crate::codegen::symbol::SymbolKind;
use crate::codegen::targets::{go, typescript};
use crate::codegen::{casing_for, layout, GeneratedFile, TargetKind};
use crate::config::normalize_ext_lang;
use crate::ir::{ExtLib, Model, Module};

/// The stable code every foreign-signature finding carries.
pub const FINDING_CODE: &str = "FX0001";

/// What a binding site is, as the frontend's `ext-bindings` listing names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The binding the finding is about, when its site was listed: what a
    /// consumer keeping the verdict across edits re-locates it by, since
    /// `span` is only good for the text the check read.
    pub site: Option<Site>,
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

/// One target's consumer tree, and how `tono gen` lays that target out:
/// the probe compiles beside the SDK's own type declarations, generated
/// under the same module mapping and casing the manifest gives the target,
/// so what the probe names is what the SDK will carry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetRoot {
    /// A directory inside the consumer tree for that language (a Go module
    /// that requires the library, a tree whose `node_modules` holds the
    /// package).
    pub dir: PathBuf,
    pub config: CodegenConfig,
    /// The manifest's casing overrides, layered on the language default.
    pub casing: Vec<(SymbolKind, CaseStyle)>,
}

impl TargetRoot {
    /// A root under the default layout: no manifest steering the target.
    pub fn plain(dir: PathBuf) -> Self {
        Self {
            dir,
            ..Self::default()
        }
    }
}

/// Where each target's library is resolved from. `None` leaves that
/// language unchecked.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LibRoots {
    pub go: Option<TargetRoot>,
    pub ts: Option<TargetRoot>,
}

/// Whether the generated SDK's type declarations stand beside the probe,
/// or why they do not (the model does not generate). A binding that names a
/// generated type needs them; without them it is listed as skipped, with
/// the reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sdk {
    Present,
    Absent(String),
}

impl Sdk {
    /// `Ok` when the SDK's types are beside the probe; else why `what` (a
    /// spelling, a parameter, a class reference) cannot be probed.
    pub fn require(&self, what: &str) -> Result<(), String> {
        match self {
            Sdk::Present => Ok(()),
            Sdk::Absent(reason) => Err(format!(
                "{what} needs the generated SDK's types, which are not beside the probe ({reason})"
            )),
        }
    }
}

/// The name a probe writes for one of the module's own shapes (`id` as the
/// IR names it), as the generated types file declares it: `Err` when the
/// types are not beside the probe, when the shape belongs to another module
/// (the check materializes the ext's own module), or when it is an entry or
/// a config (construction, not a declaration of the types file).
pub fn generated_shape(module: &Module, sdk: &Sdk, id: &str) -> Result<String, String> {
    let name = crate::codegen::entries::local_name(id);
    if let Some((owner, _)) = id.split_once('#') {
        if owner != module.name {
            return Err(format!(
                "{name} is a type of module {owner}, outside the ext's module"
            ));
        }
    }
    sdk.require(&format!("{name}, one of the module's own types,"))?;
    let shape = module
        .shapes
        .iter()
        .find(|s| crate::codegen::entries::local_name(&s.id) == name);
    if let Some(shape) = shape {
        if matches!(
            shape.kind,
            crate::ir::ShapeKind::Entry { .. } | crate::ir::ShapeKind::Config { .. }
        ) {
            return Err(format!("{name} is an entry, not a type"));
        }
    }
    crate::codegen::entries::generated_type_name(module, name)
        .ok_or_else(|| format!("{name} is not a type of module {}", module.name))
}

/// The SDK's type declarations for one target, generated in memory by the
/// pipeline `tono gen` runs (`pipeline::generate_types`): each module's
/// `types` file and the SDK-root files it may import, nothing that imports
/// the foreign library.
#[derive(Debug)]
pub struct GeneratedTypes {
    target: TargetKind,
    files: Vec<GeneratedFile>,
}

/// Generate the type declarations the probe compiles beside, under the
/// root's layout. `Err` is the pipeline's own refusal of the model.
pub fn generated_types(
    model: &Model,
    target: TargetKind,
    config: &CodegenConfig,
    root: &TargetRoot,
) -> Result<GeneratedTypes, String> {
    let mut casing = casing_for(target);
    for &(kind, style) in &root.casing {
        casing = casing.with(kind, style);
    }
    let files = crate::codegen::pipeline::generate_types(model, target, config, &casing)?;
    Ok(GeneratedTypes { target, files })
}

impl GeneratedTypes {
    /// Write the files under `dir`, laid out as `tono gen` lays the target
    /// out (the `<target>/` prefix dropped), unformatted: the toolchain
    /// reads them, nobody else does.
    pub fn write(&self, dir: &Path) -> std::io::Result<()> {
        for file in &self.files {
            let relative = file
                .path
                .strip_prefix(self.target.dir())
                .unwrap_or(&file.path);
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, &file.text)?;
        }
        Ok(())
    }

    /// The files' paths relative to the target root, in emission order.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .map(|f| {
                f.path
                    .strip_prefix(self.target.dir())
                    .unwrap_or(&f.path)
                    .to_path_buf()
            })
            .collect()
    }
}

/// The module path of the Go module `dir` sits in: the `module` directive of
/// the nearest `go.mod` above it, with the directory that holds it. `Err`
/// when no `go.mod` is found, which is also where `go build` would stop.
pub fn go_module_of(dir: &Path) -> Result<(String, PathBuf), String> {
    for ancestor in dir.ancestors() {
        let go_mod = ancestor.join("go.mod");
        let Ok(text) = std::fs::read_to_string(&go_mod) else {
            continue;
        };
        let path = text
            .lines()
            .map(str::trim)
            .find_map(|l| {
                l.strip_prefix("module ")
                    .map(|m| m.trim().trim_matches('"'))
            })
            .filter(|m| !m.is_empty())
            .ok_or_else(|| format!("{} declares no module path", go_mod.display()))?;
        return Ok((path.to_string(), ancestor.to_path_buf()));
    }
    Err(format!(
        "no go.mod above {} (the check needs the consumer's Go module to compile the probe in)",
        dir.display()
    ))
}

/// The import path the probe's scratch tree has inside the consumer's Go
/// module: the module path plus the scratch directory's path under the
/// module's root, which is what the generated files' own imports (the
/// support package, another module's package) are prefixed with.
fn scratch_go_module(scratch: &Path, root_dir: &Path) -> Result<String, String> {
    let (module_path, module_dir) = go_module_of(root_dir)?;
    let module_dir = std::fs::canonicalize(&module_dir).unwrap_or(module_dir);
    let scratch = std::fs::canonicalize(scratch).unwrap_or_else(|_| scratch.to_path_buf());
    let relative = scratch.strip_prefix(&module_dir).map_err(|_| {
        format!(
            "{} is not inside the Go module at {}",
            scratch.display(),
            module_dir.display()
        )
    })?;
    let mut path = module_path;
    for segment in relative.components() {
        path.push('/');
        path.push_str(&segment.as_os_str().to_string_lossy());
    }
    Ok(path)
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
    let site_of = |key: &SiteKey| -> Option<Site> {
        sites
            .iter()
            .find(|s| {
                s.ext == ext
                    && normalize_ext_lang(&s.lang) == lang
                    && s.kind == key.kind
                    && s.owner == key.owner
                    && s.name == key.name
            })
            .cloned()
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
                    Some(k) => {
                        let site = site_of(k);
                        findings.push(Finding {
                            span: site
                                .as_ref()
                                .map_or_else(|| "0:0-0".to_string(), |s| s.span.clone()),
                            message: format!(
                                "{lang} binding of {} in ext {ext}: {}",
                                k.label(),
                                e.message
                            ),
                            site,
                        })
                    }
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

/// What one check runs against: the sites to attribute findings to, the
/// roots to resolve the libraries in, and the pairs it covers.
struct Check<'a> {
    sites: &'a [Site],
    roots: &'a LibRoots,
    selection: &'a Selection,
}

/// Check every ext of `model` whose language has a root, and report.
pub fn verify(model: &Model, sites: &[Site], roots: &LibRoots) -> Result<Report, String> {
    verify_selected(model, sites, roots, &Selection::all())
}

/// [`verify`] over the selected pairs only.
pub fn verify_selected(
    model: &Model,
    sites: &[Site],
    roots: &LibRoots,
    selection: &Selection,
) -> Result<Report, String> {
    let check = Check {
        sites,
        roots,
        selection,
    };
    let mut report = Report::default();
    for module in &model.modules {
        for lib in &module.ext_libs {
            verify_lib(&mut report, model, module, lib, &check)?;
        }
    }
    Ok(report)
}

fn verify_lib(
    report: &mut Report,
    model: &Model,
    module: &Module,
    lib: &ExtLib,
    check: &Check<'_>,
) -> Result<(), String> {
    let Check {
        sites,
        roots,
        selection,
    } = *check;
    let wanted = |lang: &str| lang_declared(lib, lang) && selection.allows(&lib.name, lang);
    if wanted("rust") {
        report.unchecked.push(format!(
            "rust bindings of ext {}: reading a crate's signatures needs rustdoc JSON, nightly only",
            lib.name
        ));
    }
    if wanted("go") {
        match &roots.go {
            None => report.unchecked.push(format!(
                "go bindings of ext {}: no Go module to resolve the library in (set the go target's out in tono.toml, or pass --lib-root go=<dir>)",
                lib.name
            )),
            Some(root) => {
                let scratch = Scratch::create(&root.dir, "go").map_err(|e| e.to_string())?;
                // The generated files import each other through the scratch
                // tree's own path inside the consumer's module, so the
                // module path the manifest gives the SDK is replaced here.
                let sdk = match scratch_go_module(&scratch.dir, &root.dir) {
                    Ok(go_module) => {
                        let config = CodegenConfig {
                            go_module: Some(go_module),
                            ..root.config.clone()
                        };
                        materialize(model, TargetKind::Go, &config, root, &scratch)?
                    }
                    Err(reason) => Sdk::Absent(reason),
                };
                let canonical = modules::canonicalize(&root.config, &module.name);
                let module_dir = layout::module_dir(&canonical);
                let package = layout::package_name(&canonical);
                let probe = go::entry::verify::probe(module, lib, &sdk, package);
                let outcome = go::entry::verify::run(&scratch, &module_dir, &probe)
                    .map_err(|e| e.to_string())?;
                fold_outcome(report, outcome, &probe, sites, &lib.name, "go")?;
            }
        }
    }
    if wanted("ts") {
        match &roots.ts {
            None => report.unchecked.push(format!(
                "ts bindings of ext {}: no node_modules tree to resolve the package in (set the typescript target's out in tono.toml, or pass --lib-root ts=<dir>)",
                lib.name
            )),
            Some(root) => {
                let scratch = Scratch::create(&root.dir, "ts").map_err(|e| e.to_string())?;
                let sdk = materialize(model, TargetKind::TypeScript, &root.config, root, &scratch)?;
                let canonical = modules::canonicalize(&root.config, &module.name);
                let module_dir = layout::module_dir(&canonical);
                let probe = typescript::entry::verify::probe(module, lib, &sdk);
                let outcome = typescript::entry::verify::run(&scratch, &module_dir, &probe)
                    .map_err(|e| e.to_string())?;
                fold_outcome(report, outcome, &probe, sites, &lib.name, "ts")?;
            }
        }
    }
    Ok(())
}

/// Generate the SDK's types for `target` and write them into the scratch
/// tree. A model the pipeline refuses leaves the types absent, with the
/// refusal as the reason; an unwritable scratch tree is an error.
fn materialize(
    model: &Model,
    target: TargetKind,
    config: &CodegenConfig,
    root: &TargetRoot,
    scratch: &Scratch,
) -> Result<Sdk, String> {
    match generated_types(model, target, config, root) {
        Ok(types) => {
            types.write(&scratch.dir).map_err(|e| e.to_string())?;
            Ok(Sdk::Present)
        }
        Err(reason) => Ok(Sdk::Absent(reason)),
    }
}

pub mod select;
pub use select::Selection;

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

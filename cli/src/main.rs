//! The `tono` command line: turn a project's `.tono` sources into SDK source
//! files.
//!
//! `tono init` writes the project manifest (and a minimal native build manifest
//! per target) so the rest of the commands have something to discover.
//!
//! `tono gen` takes the IR, generates the per-target source through the engine,
//! formats each file with that language's formatter, and writes it out. The IR
//! is an internal artifact: with no argument and nothing piped in, the project's
//! own sources are compiled through the frontend (`project.root` from the
//! manifest), so generating an SDK needs no separate compile step. A file
//! argument or piped IR still wins, for callers that already hold one.
//! Generation runs in one of two modes: with `--target <list> --out <dir>` it
//! writes each target under `<dir>/<target>/` with module hooks from flags;
//! otherwise the project manifest (`--config <tono.toml>`, or one auto-discovered
//! up from the working directory) drives every enabled target under its own
//! configured `out`, applying that target's module hooks and casing overrides.
//!
//! `tono breaking` gates changes against a baseline, taking its policy from the
//! manifest's `[compat]` (flags override). The generation itself lives in the
//! testable `tono_backend` library; this binary is the IO shell around it.

mod check;
mod frontend;
mod gen;
mod gen_ext;
mod init;
mod init_ext;
mod native_manifest;
#[cfg(feature = "playground")]
mod playground;
mod preview;
mod split;

use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use tono_backend::codegen::{parse_targets, TargetKind};
use tono_backend::compat::{self, Category, Config, Severity};
use tono_backend::config as manifest;
use tono_backend::ir::decode_model;

use crate::frontend::Frontend;
use crate::preview::pipeline;
use crate::preview::pipeline::Verdict;

pub(crate) const USAGE: &str = "usage: tono (\n  \
    init [--target <list>] [--yes] [--root <path>]\n  \
    gen (--target <list> --out <dir> [--package <name>] [--flatten] [--module-remap <from>=<to>]... [--go-module <path>] | [--config <tono.toml>]) [--clean] [<ir.json>]\n    \
    (with no <ir.json> and nothing piped in, the project's .tono sources are compiled;\n    \
     --clean also removes generated files this run did not produce)\n  \
    check <file.tono> [--lib-root <lang>=<dir>]... [--config <tono.toml>] [--module <name>]\n    \
    (an ext block's bindings are checked against the library in that target's out dir, or in --lib-root)\n  \
    fmt <file.tono>\n  \
    preview <file.tono> --target <list> [--out <dir>] [--watch|--once]\n  \
    playground [--port <n>] [--no-open]\n  \
    breaking [<ir.json>] [--baseline <ref>] [--baseline-path <path>] [--config <cfg.json>] [--level <cat>=<sev>]... [--allow <key>]...\n  \
    split --branch <name> [--config <tono.toml>] [--ref <committish>] [<ir.json>]\n  \
    version)";

/// The project manifest's conventional filename, auto-discovered by walking up
/// from the working directory when no explicit path is given.
pub(crate) const MANIFEST_NAME: &str = "tono.toml";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("init") => init::run(&args[2..]),
        Some("gen") => gen::run(&args[2..]),
        Some("check") => run_check(&args[2..]),
        Some("fmt") => run_frontend("fmt", &args[2..]),
        Some("preview") => run_preview(&args[2..]),
        #[cfg(feature = "playground")]
        Some("playground") => playground::run(&args[2..]),
        #[cfg(not(feature = "playground"))]
        Some("playground") => Err("this build has no playground (feature disabled)".to_string()),
        Some("breaking") => run_breaking(&args[2..]),
        Some("split") => split::run(&args[2..]),
        Some("version") | None => {
            println!("tono {}", tono_backend::version());
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}\n{USAGE}")),
    }
}

/// Walk up from the working directory looking for `tono.toml`.
pub(crate) fn discover_manifest() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    for dir in cwd.ancestors() {
        let candidate = dir.join(MANIFEST_NAME);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "no {MANIFEST_NAME} found (searched up from {}); pass --config, or --target/--out\n{USAGE}",
        cwd.display()
    ))
}

/// Relay a source-level subcommand (`check`, `fmt`) to the frontend, inheriting
/// its stdio and exit code. The frontend owns parsing and typechecking, so these
/// are thin passthroughs: it prints diagnostics or the formatted source itself.
/// `tono check`: the frontend's diagnostics first (a rejected source ends
/// here, with its exit code), then every foreign binding checked against
/// its library. Findings print like the frontend's own diagnostics and fail
/// the check the same way.
fn run_check(args: &[String]) -> Result<(), String> {
    let parsed = check::parse_args(args)?;
    let path = parsed
        .path
        .clone()
        .ok_or(format!("missing <file.tono>\n{USAGE}"))?;
    run_frontend("check", std::slice::from_ref(&path))?;
    if check::check_bindings(Path::new(&path), &parsed)? {
        eprintln!("ok: {path}");
        Ok(())
    } else {
        std::process::exit(1);
    }
}

fn run_frontend(sub: &str, args: &[String]) -> Result<(), String> {
    let program = frontend::resolve_program();
    let status = Command::new(&program)
        .arg(sub)
        .args(args)
        .status()
        .map_err(|e| {
            format!("could not run {program} ({e}); set TONO_FRONTEND to the frontend binary")
        })?;
    if status.success() {
        // A clean check prints nothing of its own; `run_check` says so once
        // the bindings are checked too. `fmt` writes the formatted source,
        // and stdout is its result.
        Ok(())
    } else {
        // Mirror the frontend's exit code (1 for diagnostics, 2 for usage).
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// The preview: parse the source to IR (frontend), render the SDK for each
/// requested target, and compile-check the rendered code with that target's
/// toolchain. On a terminal it opens the interactive split-pane (watching the
/// file, `Tab` cycling the requested targets); piped or with `--once` it prints
/// one pass per target instead. `--watch` is the non-interactive re-render loop.
fn run_preview(args: &[String]) -> Result<(), String> {
    let mut path: Option<String> = None;
    let mut target_csv: Option<String> = None;
    let mut out: Option<String> = None;
    let mut watch = false;
    let mut once = false;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => target_csv = Some(flag_value(args, &mut i, "--target")?),
            "--out" => out = Some(flag_value(args, &mut i, "--out")?),
            "--watch" => watch = true,
            "--once" => once = true,
            flag if flag.starts_with("--") => return Err(format!("unknown flag: {flag}\n{USAGE}")),
            p if path.is_none() => path = Some(p.to_string()),
            p => return Err(format!("unexpected extra argument: {p}\n{USAGE}")),
        }
        i += 1;
    }

    let path = path.ok_or("missing <file.tono>")?;
    let targets = parse_targets(&target_csv.ok_or("missing --target")?)?;
    // The scaffold base; per-target subdirectories keep each toolchain's
    // incremental cache so a re-check after a save is a warm rebuild.
    let base = out.map(PathBuf::from).unwrap_or_else(|| {
        std::env::temp_dir().join(format!("tono-preview-{}", std::process::id()))
    });

    if watch {
        return run_preview_watch(&path, &targets, &base);
    }
    // The split-pane needs a real terminal; piped output (CI, shell pipelines)
    // gets the plain printed pass so `tono preview | less` and tests just work.
    if !once && std::io::stdout().is_terminal() {
        return launch_tui(PathBuf::from(path), targets, base);
    }
    match preview_pass(&path, &targets, &base)? {
        true => Ok(()),
        false => Err("preview compile-check failed".to_string()),
    }
}

/// The native checker's tool label for a target, as the printed report names it.
fn checker_tool(target: TargetKind) -> &'static str {
    match target {
        TargetKind::Rust => "cargo check",
        TargetKind::Go => "go build",
        TargetKind::TypeScript => "tsc",
    }
}

/// One printed pass over every target, through the same pipeline the split-pane
/// uses. Returns whether all present toolchains reported success (a skipped
/// toolchain is not a failure). Anything upstream of the compile-check (an
/// unreadable file, a source the frontend rejects, a model the generator
/// rejects) is a hard error and aborts the pass.
fn preview_pass(path: &str, targets: &[TargetKind], base: &Path) -> Result<bool, String> {
    let frontend = Frontend::from_env();
    let mut all_ok = true;
    for &target in targets {
        println!("== target: {} ==", target.dir());
        let scratch = base.join(target.dir());
        let snapshot = pipeline::run(&frontend, Path::new(path), target, &scratch);
        match snapshot.verdict {
            Verdict::Compiles => {
                println!("{}", snapshot.generated);
                println!("compiles: yes ({})", checker_tool(target));
            }
            Verdict::DoesNotCompile(diagnostics) => {
                println!("{}", snapshot.generated);
                println!("compiles: no ({})", checker_tool(target));
                print!("{diagnostics}");
                all_ok = false;
            }
            Verdict::ToolchainMissing(program) => {
                println!("{}", snapshot.generated);
                println!("compile-check: {program} not found (skipped)");
            }
            verdict => {
                let mut msg = verdict.headline();
                if let Some(detail) = verdict.detail() {
                    msg.push('\n');
                    msg.push_str(detail);
                }
                return Err(msg);
            }
        }
    }
    Ok(all_ok)
}

/// Watch the source and re-run the printed pass on every change. A transient
/// error (a save caught mid-edit) is printed and the watch continues rather than
/// exiting. The watch ends cleanly when the file disappears: that is the edit
/// session moving on (rename or delete), not a save to re-render.
fn run_preview_watch(path: &str, targets: &[TargetKind], base: &Path) -> Result<(), String> {
    use std::time::Duration;
    println!("watching {path} (Ctrl-C to stop; deleting the file ends the watch)");
    let mut last: Option<std::time::SystemTime> = None;
    loop {
        match fs::metadata(path).and_then(|m| m.modified()) {
            Err(_) if last.is_some() => {
                println!("{path} is gone; watch stopped");
                return Ok(());
            }
            Err(_) => {}
            Ok(stamp) => {
                if Some(stamp) != last {
                    last = Some(stamp);
                    println!("\n--- preview {path} ---");
                    if let Err(e) = preview_pass(path, targets, base) {
                        eprintln!("{e}");
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Consume the value that follows a flag, advancing the cursor past it.
pub(crate) fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Launch the interactive split-pane. Present only when built with the `preview`
/// feature (the default).
#[cfg(feature = "preview")]
fn launch_tui(source: PathBuf, targets: Vec<TargetKind>, base: PathBuf) -> Result<(), String> {
    preview::tui::run(source, targets, base).map_err(|e| format!("preview: {e}"))
}

/// Without the `preview` feature the terminal UI is not compiled in; the lean
/// binary degrades to the printed pass, so `tono preview` still answers.
#[cfg(not(feature = "preview"))]
fn launch_tui(source: PathBuf, targets: Vec<TargetKind>, base: PathBuf) -> Result<(), String> {
    match preview_pass(&source.to_string_lossy(), &targets, &base)? {
        true => Ok(()),
        false => Err("preview compile-check failed".to_string()),
    }
}

/// The compat gate: diff the current IR against a baseline committed at a git ref,
/// classify each change, apply the configured severity, and fail the build on any
/// `error`-level break. The baseline IR is read straight from git (`git show
/// <ref>:<path>`), so the check needs only the repository, no rebuild of the past.
fn run_breaking(args: &[String]) -> Result<(), String> {
    let mut current_path: Option<String> = None;
    let mut baseline_ref: Option<String> = None;
    let mut baseline_path: Option<String> = None;
    let mut config_path: Option<String> = None;
    let mut levels: Vec<(Category, Severity)> = Vec::new();
    let mut allow: Vec<String> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--baseline" => baseline_ref = Some(flag_value(args, &mut i, "--baseline")?),
            "--baseline-path" => baseline_path = Some(flag_value(args, &mut i, "--baseline-path")?),
            "--config" => config_path = Some(flag_value(args, &mut i, "--config")?),
            "--level" => levels.push(parse_level(&flag_value(args, &mut i, "--level")?)?),
            "--allow" => allow.push(flag_value(args, &mut i, "--allow")?),
            // Reject a mistyped flag rather than silently treating it as the IR
            // path, and reject a second positional so an accidental extra argument
            // does not overwrite the first.
            flag if flag.starts_with("--") => return Err(format!("unknown flag: {flag}\n{USAGE}")),
            path if current_path.is_none() => current_path = Some(path.to_string()),
            path => return Err(format!("unexpected extra argument: {path}\n{USAGE}")),
        }
        i += 1;
    }

    // The policy layers, weakest to strongest: the project manifest's [compat] if
    // one is discoverable, then a JSON config file, then --level flags on top.
    let manifest_file_path = discover_manifest().ok();
    let manifest_loaded = match &manifest_file_path {
        Some(path) => Some(manifest::Config::load(path)?),
        None => None,
    };
    let manifest_compat = manifest_loaded.as_ref().map(|c| c.compat.clone());
    let mut config = Config::default();
    if let Some(compat_cfg) = &manifest_compat {
        apply_manifest_severities(&mut config, compat_cfg);
    }
    if let Some(path) = &config_path {
        let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let json = Config::from_json(&text).map_err(|e| format!("{path}: {e}"))?;
        config.severity.extend(json.severity);
        config.allow.extend(json.allow);
    }
    for (category, severity) in levels {
        config.severity.insert(category, severity);
    }
    config.allow.extend(allow);

    // The current IR comes from the file argument, or stdin when omitted.
    let current_json = match &current_path {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => read_stdin()?,
    };
    let current = decode_model(&current_json)?;

    // The baseline IR is the same committed file at the given ref (default: the
    // last release tag). Its path defaults to the current file, so a check usually
    // needs only `--baseline <ref>`.
    let git_path = baseline_path
        .or_else(|| current_path.clone())
        .ok_or("--baseline-path is required when the current IR comes from stdin")?;
    // The baseline ref: --baseline wins, else the manifest's [compat].baseline
    // (a concrete git ref), else the last release tag.
    let git_ref = match baseline_ref {
        Some(r) => r,
        None => match manifest_compat
            .as_ref()
            .and_then(|c| baseline_ref_from_manifest(&c.baseline))
        {
            Some(r) => r,
            None => last_tag()?,
        },
    };
    let baseline_json = git_show(&git_ref, &git_path)?;
    let baseline =
        decode_model(&baseline_json).map_err(|e| format!("baseline {git_ref}:{git_path}: {e}"))?;

    let mut report = compat::diff(&baseline, &current);
    if let Some(manifest) = &manifest_loaded {
        report.changes.extend(ext_version_changes(
            &git_ref,
            manifest_file_path.as_deref().unwrap(),
            &manifest.ext_versions,
        ));
    }
    print_report(&report, &config, &git_ref);

    if report.worst(&config) == Severity::Error {
        return Err(format!("breaking changes detected against {git_ref}"));
    }
    Ok(())
}

/// Seed the compat policy from the manifest's `[compat]` severities. The
/// manifest exposes the three enforceable levels; the additive-safe level keeps
/// its `off` default (it is not a break).
fn apply_manifest_severities(config: &mut Config, compat: &manifest::Compat) {
    for (category, severity) in [
        (Category::WireBreaking, compat.wire_breaking),
        (Category::SourceBreaking, compat.source_breaking),
        (Category::Behavioral, compat.behavioral),
    ] {
        config
            .severity
            .insert(category, to_compat_severity(severity));
    }
}

/// Map the manifest's severity enum onto the compat checker's.
fn to_compat_severity(severity: manifest::Severity) -> Severity {
    match severity {
        manifest::Severity::Error => Severity::Error,
        manifest::Severity::Warn => Severity::Warn,
        manifest::Severity::Off => Severity::Off,
    }
}

/// Translate `[compat].baseline` into a concrete git ref. `git:last-tag` (the
/// default) means "fall through to the last tag", so it yields `None`; any other
/// value is a ref, with an optional `git:` prefix stripped.
fn baseline_ref_from_manifest(baseline: &str) -> Option<String> {
    match baseline {
        "git:last-tag" => None,
        other => Some(other.strip_prefix("git:").unwrap_or(other).to_string()),
    }
}

/// Parse a `--level <category>=<severity>` override.
fn parse_level(spec: &str) -> Result<(Category, Severity), String> {
    let (cat, sev) = spec
        .split_once('=')
        .ok_or_else(|| format!("--level expects <category>=<severity>, got {spec:?}"))?;
    let category = Category::parse(cat).ok_or_else(|| format!("unknown level: {cat}"))?;
    let severity = Severity::parse(sev).ok_or_else(|| format!("unknown severity: {sev}"))?;
    Ok((category, severity))
}

/// Read a file as it was committed at a git ref: `git show <ref>:<path>`.
fn git_show(git_ref: &str, path: &str) -> Result<String, String> {
    let spec = format!("{git_ref}:{path}");
    let out = Command::new("git")
        .args(["show", &spec])
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git show {spec}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("git show {spec}: output is not utf-8: {e}"))
}

/// The repository's top-level directory, so an absolute path (like the one
/// [`discover_manifest`] returns) can be turned into the repo-root-relative
/// spelling `git show <ref>:<path>` requires.
fn git_repo_root() -> Result<PathBuf, String> {
    let out = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git rev-parse --show-toplevel: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
    ))
}

/// The `[ext.*]` version-pin changes between the baseline manifest (fetched
/// the same way the baseline IR is, `git show <ref>:<path>`) and the current
/// one already on disk. A project adopting `[ext]` for the first time has no
/// manifest at the baseline ref: that `git show` failure is not an error for
/// this check, it just means there is nothing to compare against yet.
fn ext_version_changes(
    git_ref: &str,
    manifest_path: &Path,
    current: &BTreeMap<String, BTreeMap<String, String>>,
) -> Vec<compat::Change> {
    let Ok(root) = git_repo_root() else {
        return Vec::new();
    };
    let Ok(rel) = manifest_path.strip_prefix(&root) else {
        return Vec::new();
    };
    let Some(rel) = rel.to_str() else {
        return Vec::new();
    };
    let baseline = match git_show(git_ref, rel) {
        Ok(text) => match manifest::Config::from_toml_str(&text) {
            Ok(cfg) => cfg.ext_versions,
            Err(_) => return Vec::new(),
        },
        Err(_) => return Vec::new(),
    };
    tono_backend::compat_ext::diff_ext_versions(&baseline, current)
}

/// The most recent tag, the default baseline ref (compare against the last
/// released version).
fn last_tag() -> Result<String, String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--abbrev=0"])
        .output()
        .map_err(|e| format!("running git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "no --baseline ref given and no tag to default to: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Print each reportable change with its effective severity, then a one-line
/// summary. Changes whose effective severity is `off` (safe/additive, or
/// allowlisted) are omitted from the listing but counted as suppressed.
fn print_report(report: &compat::Report, config: &Config, git_ref: &str) {
    let mut errors = 0;
    let mut warnings = 0;
    let mut suppressed = 0;
    for change in &report.changes {
        match config.severity_of(change) {
            Severity::Error => {
                errors += 1;
                println!("error: {} ({})", change.key, change.detail);
            }
            Severity::Warn => {
                warnings += 1;
                println!("warn: {} ({})", change.key, change.detail);
            }
            Severity::Off => suppressed += 1,
        }
    }
    println!(
        "against {git_ref}: {} change(s) - {errors} error, {warnings} warn, {suppressed} ok/allowed",
        report.changes.len()
    );
}

pub(crate) fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

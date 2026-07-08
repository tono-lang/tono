//! The `tono` command line: turn IR JSON into SDK source files.
//!
//! `tono gen --target <list> --out <dir> [<ir.json>]` reads the IR (from the file
//! argument, or stdin when omitted), decodes it, generates the per-target source
//! through the engine, formats each file with that language's formatter, and
//! writes it under `<dir>/<target>/`. The generation itself lives in the testable
//! `tono_backend::codegen` library; this binary is the IO shell around it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use tono_backend::codegen::{generate, parse_targets, CodegenConfig, Formatter, TargetKind};
use tono_backend::compat::{self, Category, Config, Severity};
use tono_backend::ir::decode_model;

const USAGE: &str = "usage: tono (\n  \
    gen --target <list> --out <dir> [--flatten] [--module-remap <from>=<to>]... [<ir.json>]\n  \
    breaking [<ir.json>] --baseline <ref> [--baseline-path <path>] [--config <cfg.json>] [--level <cat>=<sev>]... [--allow <key>]...\n  \
    version)";

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
        Some("gen") => run_gen(&args[2..]),
        Some("breaking") => run_breaking(&args[2..]),
        Some("version") | None => {
            println!("tono {}", tono_backend::version());
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}\n{USAGE}")),
    }
}

fn run_gen(args: &[String]) -> Result<(), String> {
    let mut targets_csv: Option<String> = None;
    let mut out: Option<String> = None;
    let mut ir_path: Option<String> = None;
    let mut config = CodegenConfig::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => targets_csv = Some(flag_value(args, &mut i, "--target")?),
            "--out" => out = Some(flag_value(args, &mut i, "--out")?),
            // Collapse the module hierarchy into flat single-segment packages.
            "--flatten" => config.flatten = true,
            // Rewrite a module prefix, e.g. --module-remap payments=billing.
            "--module-remap" => {
                config
                    .remap
                    .push(parse_remap(&flag_value(args, &mut i, "--module-remap")?)?)
            }
            path => ir_path = Some(path.to_string()),
        }
        i += 1;
    }

    let targets = parse_targets(&targets_csv.ok_or("missing --target")?)?;
    let out_root = PathBuf::from(out.ok_or("missing --out")?);

    let json = match &ir_path {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => read_stdin()?,
    };
    let model = decode_model(&json)?;

    for file in generate(&model, &targets, &config) {
        let formatted = formatter_for(file.target).run(&file.text).text;
        write_file(&out_root.join(&file.path), &formatted)?;
    }
    Ok(())
}

/// Parse a `--module-remap from=to` value into its `(from, to)` pair.
fn parse_remap(value: &str) -> Result<(String, String), String> {
    value
        .split_once('=')
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .ok_or_else(|| format!("--module-remap expects <from>=<to>, got: {value}"))
}

/// Consume the value that follows a flag, advancing the cursor past it.
fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
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

    // The policy: a JSON config file if given, then flag overrides on top.
    let mut config = match &config_path {
        Some(path) => {
            let text = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            Config::from_json(&text).map_err(|e| format!("{path}: {e}"))?
        }
        None => Config::default(),
    };
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
    let git_ref = match baseline_ref {
        Some(r) => r,
        None => last_tag()?,
    };
    let baseline_json = git_show(&git_ref, &git_path)?;
    let baseline =
        decode_model(&baseline_json).map_err(|e| format!("baseline {git_ref}:{git_path}: {e}"))?;

    let report = compat::diff(&baseline, &current);
    print_report(&report, &config, &git_ref);

    if report.worst(&config) == Severity::Error {
        return Err(format!("breaking changes detected against {git_ref}"));
    }
    Ok(())
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

/// The formatter for a target. A missing binary degrades to the rough text (the
/// [`Formatter`] never fails), so generation works without the toolchain, just
/// less prettily.
fn formatter_for(target: TargetKind) -> Formatter {
    match target {
        TargetKind::Rust => Formatter::new("rustfmt", vec!["--edition".into(), "2021".into()]),
        TargetKind::Go => Formatter::new("gofmt", vec![]),
        TargetKind::TypeScript => {
            Formatter::new("prettier", vec!["--parser".into(), "typescript".into()])
        }
    }
}

fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn write_file(dest: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

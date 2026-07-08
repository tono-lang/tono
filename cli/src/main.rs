//! The `tono` command line: turn IR JSON into SDK source files.
//!
//! `tono gen` reads the IR (from the file argument, or stdin when omitted),
//! decodes it, generates the per-target source through the engine, formats each
//! file with that language's formatter, and writes it out. It runs in one of two
//! modes: with `--target <list> --out <dir>` it writes each target under
//! `<dir>/<target>/` with module hooks from flags; otherwise the project manifest
//! (`--config <tono.toml>`, or one auto-discovered up from the working directory)
//! drives every enabled target under its own configured `out`, applying that
//! target's module hooks and casing overrides. `tono breaking` gates changes
//! against a baseline, taking its policy from the manifest's `[compat]` (flags
//! override). The generation itself lives in the testable `tono_backend` library;
//! this binary is the IO shell around it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use tono_backend::codegen::{
    casing_for, check_go_layout, generate, generate_target, parse_targets, CasingConfig,
    CodegenConfig, Formatter, TargetKind,
};
use tono_backend::compat::{self, Category, Config, Severity};
use tono_backend::config as manifest;
use tono_backend::ir::{decode_model, Model};

const USAGE: &str = "usage: tono (\n  \
    gen (--target <list> --out <dir> [--flatten] [--module-remap <from>=<to>]... [--go-module <path>] | [--config <tono.toml>]) [<ir.json>]\n  \
    breaking [<ir.json>] [--baseline <ref>] [--baseline-path <path>] [--config <cfg.json>] [--level <cat>=<sev>]... [--allow <key>]...\n  \
    version)";

/// The project manifest's conventional filename, auto-discovered by walking up
/// from the working directory when no explicit path is given.
const MANIFEST_NAME: &str = "tono.toml";

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
    let mut manifest_path: Option<String> = None;
    let mut ir_path: Option<String> = None;
    let mut config = CodegenConfig::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => targets_csv = Some(flag_value(args, &mut i, "--target")?),
            "--out" => out = Some(flag_value(args, &mut i, "--out")?),
            // The project manifest, when not driving generation from flags.
            "--config" => manifest_path = Some(flag_value(args, &mut i, "--config")?),
            // Collapse the module hierarchy into flat single-segment packages.
            "--flatten" => config.flatten = true,
            // Rewrite a module prefix, e.g. --module-remap payments=billing.
            "--module-remap" => {
                config
                    .remap
                    .push(parse_remap(&flag_value(args, &mut i, "--module-remap")?)?)
            }
            // The generated Go SDK's module path, prefixed onto cross-package
            // imports (Go has no relative imports).
            "--go-module" => config.go_module = Some(flag_value(args, &mut i, "--go-module")?),
            path => ir_path = Some(path.to_string()),
        }
        i += 1;
    }

    // Explicit --target/--out selects the flag path (a single output root, module
    // hooks from flags); otherwise the project manifest drives generation.
    if targets_csv.is_some() || out.is_some() {
        gen_from_flags(&targets_csv, &out, &config, &ir_path)
    } else {
        gen_from_manifest(manifest_path.as_deref(), &ir_path)
    }
}

/// Flag path: an explicit target list and one output root, with the module hooks
/// taken from flags. Each file lands under `<out>/<target>/`.
fn gen_from_flags(
    targets_csv: &Option<String>,
    out: &Option<String>,
    config: &CodegenConfig,
    ir_path: &Option<String>,
) -> Result<(), String> {
    let targets = parse_targets(targets_csv.as_deref().ok_or("missing --target")?)?;
    let out_root = PathBuf::from(out.as_deref().ok_or("missing --out")?);
    let model = read_ir(ir_path)?;
    // Fail loud on a Go layout that could not compile (a multi-module SDK with no
    // module path, or two modules mapping to the same package) instead of writing
    // silently-broken source.
    check_go_layout(&model, &targets, config)?;
    for file in generate(&model, &targets, config)? {
        let formatted = formatter_for(file.target).run(&file.text).text;
        write_file(&out_root.join(&file.path), &formatted)?;
    }
    Ok(())
}

/// Manifest path: resolve the project config, then generate every enabled target
/// under its own `out` (relative to the manifest's directory), applying that
/// target's module hooks (flatten/remap/package) and casing overrides.
fn gen_from_manifest(config_path: Option<&str>, ir_path: &Option<String>) -> Result<(), String> {
    let manifest_file = match config_path {
        Some(path) => PathBuf::from(path),
        None => discover_manifest()?,
    };
    let base = manifest_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let cfg = manifest::Config::load(&manifest_file)?;
    if cfg.targets.is_empty() {
        return Err(format!(
            "{}: no enabled targets (supported: go, rust, typescript)",
            manifest_file.display()
        ));
    }

    let model = read_ir(ir_path)?;
    for target in &cfg.targets {
        let codegen = codegen_config_for(target);
        let casing = resolved_casing(target);
        check_go_layout(&model, &[target.kind], &codegen)?;
        for file in generate_target(&model, target.kind, &codegen, &casing)? {
            let formatted = formatter_for(file.target).run(&file.text).text;
            // Paths carry the `<target-dir>/` prefix; strip it so the files land
            // directly under the target's configured `out`.
            let rel = file
                .path
                .strip_prefix(target.kind.dir())
                .unwrap_or(&file.path);
            write_file(&base.join(&target.out).join(rel), &formatted)?;
        }
    }
    Ok(())
}

/// Build a target's module-mapping config from its manifest entry. RFC-0016 says
/// the most specific remap wins, so the table is ordered longest-prefix-first
/// before the engine's first-match rule sees it. `package` becomes the Go module
/// path; other targets ignore it.
fn codegen_config_for(target: &manifest::ResolvedTarget) -> CodegenConfig {
    let mut remap: Vec<(String, String)> = target
        .module_remap
        .iter()
        .map(|(from, to)| (from.clone(), to.clone()))
        .collect();
    remap.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    CodegenConfig {
        flatten: matches!(target.module_mapping, manifest::ModuleMapping::Flat),
        remap,
        go_module: target.package.clone(),
    }
}

/// Layer a target's manifest casing overrides on the language's idiomatic default.
fn resolved_casing(target: &manifest::ResolvedTarget) -> CasingConfig {
    let mut casing = casing_for(target.kind);
    for &(kind, style) in &target.casing {
        casing = casing.with(kind, style);
    }
    casing
}

/// Walk up from the working directory looking for `tono.toml`.
fn discover_manifest() -> Result<PathBuf, String> {
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

/// Read the IR JSON from the path argument, or stdin when omitted, and decode it.
fn read_ir(ir_path: &Option<String>) -> Result<Model, String> {
    let json = match ir_path {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => read_stdin()?,
    };
    decode_model(&json)
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

    // The policy layers, weakest to strongest: the project manifest's [compat] if
    // one is discoverable, then a JSON config file, then --level flags on top.
    let manifest_compat = match discover_manifest().ok() {
        Some(path) => Some(manifest::Config::load(&path)?.compat),
        None => None,
    };
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

    let report = compat::diff(&baseline, &current);
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

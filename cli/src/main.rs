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

mod frontend;
mod init;
mod preview;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use tono_backend::codegen::{
    casing_for, check_layout, generate, generate_target, parse_targets, CasingConfig, CheckOptions,
    CodegenConfig, Formatter, TargetKind,
};
use tono_backend::compat::{self, Category, Config, Severity};
use tono_backend::config as manifest;
use tono_backend::ir::{decode_model, Model};

use crate::frontend::Frontend;
use crate::preview::pipeline;
use crate::preview::pipeline::Verdict;

const USAGE: &str = "usage: tono (\n  \
    init [--target <list>] [--yes] [--root <path>]\n  \
    gen (--target <list> --out <dir> [--flatten] [--module-remap <from>=<to>]... [--go-module <path>] | [--config <tono.toml>]) [<ir.json>]\n  \
    check <file.tono>\n  \
    fmt <file.tono>\n  \
    preview <file.tono> --target <list> [--out <dir>] [--watch|--once]\n  \
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
        Some("init") => init::run(&args[2..]),
        Some("gen") => run_gen(&args[2..]),
        Some("check") => run_frontend("check", &args[2..]),
        Some("fmt") => run_frontend("fmt", &args[2..]),
        Some("preview") => run_preview(&args[2..]),
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
    check_layout(&model, &targets, config)?;
    for file in generate(&model, &targets, config)? {
        let formatted = Formatter::for_output(file.target, &file.path)
            .run(&file.text)
            .text;
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
        check_layout(&model, &[target.kind], &codegen)?;
        for file in generate_target(&model, target.kind, &codegen, &casing)? {
            let formatted = Formatter::for_output(file.target, &file.path)
                .run(&file.text)
                .text;
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

/// Relay a source-level subcommand (`check`, `fmt`) to the frontend, inheriting
/// its stdio and exit code. The frontend owns parsing and typechecking, so these
/// are thin passthroughs: it prints diagnostics or the formatted source itself.
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
    let options = preview_options();

    if watch {
        return run_preview_watch(&path, &targets, &base, &options);
    }
    // The split-pane needs a real terminal; piped output (CI, shell pipelines)
    // gets the plain printed pass so `tono preview | less` and tests just work.
    use std::io::IsTerminal;
    if !once && std::io::stdout().is_terminal() {
        return launch_tui(PathBuf::from(path), targets, base, options);
    }
    match preview_pass(&path, &targets, &base, &options)? {
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
fn preview_pass(
    path: &str,
    targets: &[TargetKind],
    base: &Path,
    options: &CheckOptions,
) -> Result<bool, String> {
    let frontend = Frontend::from_env();
    let mut all_ok = true;
    for &target in targets {
        println!("== target: {} ==", target.dir());
        let scratch = base.join(target.dir());
        let snapshot = pipeline::run(&frontend, Path::new(path), target, &scratch, options);
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
fn run_preview_watch(
    path: &str,
    targets: &[TargetKind],
    base: &Path,
    options: &CheckOptions,
) -> Result<(), String> {
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
                    if let Err(e) = preview_pass(path, targets, base, options) {
                        eprintln!("{e}");
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// Consume the value that follows a flag, advancing the cursor past it.
fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// Launch the interactive split-pane. Present only when built with the `preview`
/// feature (the default).
#[cfg(feature = "preview")]
fn launch_tui(
    source: PathBuf,
    targets: Vec<TargetKind>,
    base: PathBuf,
    options: CheckOptions,
) -> Result<(), String> {
    preview::tui::run(source, targets, base, options).map_err(|e| format!("preview: {e}"))
}

/// Without the `preview` feature the terminal UI is not compiled in; the lean
/// binary degrades to the printed pass, so `tono preview` still answers.
#[cfg(not(feature = "preview"))]
fn launch_tui(
    source: PathBuf,
    targets: Vec<TargetKind>,
    base: PathBuf,
    options: CheckOptions,
) -> Result<(), String> {
    match preview_pass(&source.to_string_lossy(), &targets, &base, &options)? {
        true => Ok(()),
        false => Err("preview compile-check failed".to_string()),
    }
}

/// Preview knobs from the environment: `TONO_HTTP_RUNTIME_TS` maps the hand-written
/// TypeScript runtime so a generated client type-checks against it.
fn preview_options() -> CheckOptions {
    CheckOptions {
        ts_runtime: std::env::var_os("TONO_HTTP_RUNTIME_TS").map(PathBuf::from),
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

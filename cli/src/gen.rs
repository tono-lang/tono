//! `tono gen`: the SDK source for every enabled target, written to disk.
//!
//! The engine ([`tono_backend`]) is pure, a model in and rough source out. This
//! module is the IO shell around it and owns what needs the filesystem: where
//! the IR comes from (an argument, a pipe, or the project's own sources through
//! the frontend), which formatter each file goes through, where it lands, what
//! the run reports, and what a sweep is allowed to remove.

use std::fs;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use tono_backend::codegen::{
    casing_for, check_layout, generate, generate_target, is_generated, parse_targets, CasingConfig,
    CodegenConfig, Formatter, TargetKind, Warning,
};
use tono_backend::config as manifest;
use tono_backend::ir::{decode_model, Model};

use crate::frontend::{Frontend, FrontendError};
use crate::{discover_manifest, flag_value, gen_ext, init, native_manifest, read_stdin};

pub fn run(args: &[String]) -> Result<(), String> {
    let mut targets_csv: Option<String> = None;
    let mut out: Option<String> = None;
    let mut manifest_path: Option<String> = None;
    let mut ir_path: Option<String> = None;
    let mut clean = false;
    let mut package: Option<String> = None;
    let mut config = CodegenConfig::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => targets_csv = Some(flag_value(args, &mut i, "--target")?),
            "--out" => out = Some(flag_value(args, &mut i, "--out")?),
            // The crate/package name a scaffolded native manifest declares
            // (flag path only; the project manifest carries its own).
            "--package" => package = Some(flag_value(args, &mut i, "--package")?),
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
            // Sweep each output directory of generated files this run did not
            // produce, so a renamed or deleted module leaves nothing behind.
            "--clean" => clean = true,
            path => ir_path = Some(path.to_string()),
        }
        i += 1;
    }

    // Explicit --target/--out selects the flag path (a single output root, module
    // hooks from flags); otherwise the project manifest drives generation.
    if targets_csv.is_some() || out.is_some() {
        gen_from_flags(&targets_csv, &out, &package, &config, &ir_path, clean)
    } else {
        if package.is_some() {
            eprintln!("--package is ignored: the project manifest names each target's package");
        }
        gen_from_manifest(manifest_path.as_deref(), &ir_path, clean)
    }
}

/// Flag path: an explicit target list and one output root, with the module hooks
/// taken from flags. Each file lands under `<out>/<target>/`, with the native
/// build manifest scaffolded when absent so the output builds as it lands.
fn gen_from_flags(
    targets_csv: &Option<String>,
    out: &Option<String>,
    package: &Option<String>,
    config: &CodegenConfig,
    ir_path: &Option<String>,
    clean: bool,
) -> Result<(), String> {
    let targets = parse_targets(targets_csv.as_deref().ok_or("missing --target")?)?;
    let out_root = PathBuf::from(out.as_deref().ok_or("missing --out")?);
    // No manifest in this mode, so sources (when it comes to that) are the
    // working directory.
    let model = read_ir(ir_path, Path::new("."))?;
    // Fail loud on a Go layout that could not compile (a multi-module SDK with no
    // module path, or two modules mapping to the same package) instead of writing
    // silently-broken source.
    check_layout(&model, &targets, config)?;
    let package = package.clone().unwrap_or_else(|| "sdk".to_string());
    let mut warned = std::collections::HashSet::new();
    for target in &targets {
        let mut written = Vec::new();
        for file in generate(&model, &[*target], config)? {
            let formatted = apply_format(
                &Formatter::for_output(file.target, &file.path),
                &file.path,
                &file.text,
                &mut warned,
            )?;
            let dest = out_root.join(&file.path);
            write_generated(&dest, file.target, &formatted)?;
            written.push(dest);
        }
        let out_dir = out_root.join(target.dir());
        // Go's package is its module path; --go-module names it when given.
        let native_package = match target {
            TargetKind::Go => config
                .go_module
                .clone()
                .unwrap_or_else(|| init::default_package(TargetKind::Go, &package)),
            _ => package.clone(),
        };
        // No manifest means no ext version pins, so only the emitted source's
        // own requirements are declared here.
        native_manifest::ensure(*target, &out_dir, &native_package, &model, &[])?;
        // Each target owns `<out>/<target>/`; a sweep stays inside it.
        report_target(*target, &out_dir, &written, clean)?;
    }
    Ok(())
}

/// Manifest path: resolve the project config, then generate every enabled target
/// under its own `out` (relative to the manifest's directory), applying that
/// target's module hooks (flatten/remap/package) and casing overrides.
fn gen_from_manifest(
    config_path: Option<&str>,
    ir_path: &Option<String>,
    clean: bool,
) -> Result<(), String> {
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

    // `project.root` is where the `.tono` sources live, relative to the
    // manifest, which is exactly what compiling the project from source needs.
    let source_root = base.join(&cfg.project.root);
    let model = read_ir(ir_path, &source_root)?;

    // An ext lib used by an enabled target with no pinned version fails the
    // whole run before anything is written, rather than partway through.
    let mut ext_deps = Vec::new();
    for target in &cfg.targets {
        ext_deps.push(gen_ext::ext_deps_for(&model, &cfg, target.kind)?);
    }

    let package_default = cfg
        .project
        .name
        .as_deref()
        .map(init::slugify)
        .unwrap_or_else(|| init::slugify(&init::project_name_default(&base)));
    for (target, deps) in cfg.targets.iter().zip(&ext_deps) {
        let out_dir = base.join(&target.out);
        let mut written = Vec::new();
        for (rel, formatted) in target_outputs(&model, target)? {
            let formatted = if target.kind == TargetKind::TypeScript
                && rel == Path::new("package.json")
                && !deps.is_empty()
            {
                gen_ext::inject_package_json_dependencies(&formatted, deps)?
            } else {
                formatted
            };
            let dest = out_dir.join(&rel);
            write_generated(&dest, target.kind, &formatted)?;
            written.push(dest);
        }
        let package = target
            .package
            .clone()
            .unwrap_or_else(|| init::default_package(target.kind, &package_default));
        native_manifest::ensure(target.kind, &out_dir, &package, &model, deps)?;
        report_target(target.kind, &out_dir, &written, clean)?;
    }
    Ok(())
}

/// The formatted files a manifest target generates: each entry is the path
/// relative to the target's `out` directory and the file's final text. This is
/// the shared generation step behind `tono gen` (which writes them under `out`)
/// and `tono split` (which projects them into a mirror).
pub(crate) fn target_outputs(
    model: &Model,
    target: &manifest::ResolvedTarget,
) -> Result<Vec<(PathBuf, String)>, String> {
    let codegen = codegen_config_for(target);
    let casing = resolved_casing(target);
    check_layout(model, &[target.kind], &codegen)?;
    let mut files = Vec::new();
    let mut warned = std::collections::HashSet::new();
    for file in generate_target(model, target.kind, &codegen, &casing)? {
        let formatted = apply_format(
            &Formatter::for_output(file.target, &file.path),
            &file.path,
            &file.text,
            &mut warned,
        )?;
        // Paths carry the `<target-dir>/` prefix; strip it so the files land
        // directly under the target's configured `out`.
        let rel = file
            .path
            .strip_prefix(target.kind.dir())
            .unwrap_or(&file.path)
            .to_path_buf();
        files.push((rel, formatted));
    }
    Ok(files)
}

/// Run the formatter over one generated file and decide what a fallback means.
/// A rejected input fails the run: the formatter parses the source, so a
/// rejection means the generator emitted invalid syntax, and broken source must
/// never reach disk looking like a successful generation. An absent formatter
/// is only an environment gap — the rough text is still correct source — so it
/// is written as-is with a note on stderr, once per missing program.
fn apply_format(
    formatter: &Formatter,
    path: &Path,
    text: &str,
    warned: &mut std::collections::HashSet<String>,
) -> Result<String, String> {
    let formatted = formatter.run(text);
    match formatted.warning {
        None => Ok(formatted.text),
        Some(Warning::FormatterUnavailable { program }) => {
            if warned.insert(program.clone()) {
                eprintln!("warning: {program} not found; writing unformatted source");
            }
            Ok(formatted.text)
        }
        Some(Warning::FormatterRejected {
            program,
            status,
            stderr,
        }) => Err(format!(
            "{}: {program} rejected the generated source (exit {}): {}\n\
             this is a bug in tono: the generator emitted invalid syntax",
            path.display(),
            status.map_or_else(|| "signal".to_string(), |code| code.to_string()),
            stderr.trim(),
        )),
    }
}

/// Report what a target produced, and when asked, clear what it no longer
/// produces. Status goes to stderr so it never mixes into a command's data
/// (`tono fmt` writes source to stdout), which also keeps it visible when the
/// output of a run is redirected.
fn report_target(
    target: TargetKind,
    out_dir: &Path,
    written: &[PathBuf],
    clean: bool,
) -> Result<(), String> {
    let removed = if clean { sweep(out_dir, written)? } else { 0 };
    eprintln!(
        "{}: {} file(s) -> {}{}",
        target.dir(),
        written.len(),
        out_dir.display(),
        match removed {
            0 => String::new(),
            n => format!(" ({n} stale removed)"),
        }
    );
    Ok(())
}

/// Delete the generated files under `out_dir` that this run did not write, then
/// the directories that leaves empty, and answer how many files went.
///
/// Only files this generator produced are eligible, identified by the banner
/// they carry ([`is_generated`]). Everything else in the output directory (a
/// `Cargo.toml`, a hand-written test, a README, the generated `package.json`
/// that carries no banner because JSON has no comments) is left untouched: the
/// output directory belongs to the user, and only what tono wrote is tono's to
/// remove.
fn sweep(out_dir: &Path, written: &[PathBuf]) -> Result<usize, String> {
    if !out_dir.is_dir() {
        return Ok(0);
    }
    let kept: std::collections::HashSet<PathBuf> = written
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .collect();
    let mut removed = 0;
    sweep_dir(out_dir, &kept, &mut removed)?;
    Ok(removed)
}

fn sweep_dir(
    dir: &Path,
    kept: &std::collections::HashSet<PathBuf>,
    removed: &mut usize,
) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if file_type.is_dir() {
            sweep_dir(&path, kept, removed)?;
            // A directory emptied by the sweep was holding only stale output
            // (a module that went away); one that still has anything stays.
            if fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()) {
                fs::remove_dir(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            }
            continue;
        }
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if kept.contains(&canonical) {
            continue;
        }
        // Unreadable or non-UTF-8 means it is not something this generator
        // wrote, so it is not this sweep's to delete.
        if fs::read_to_string(&path).is_ok_and(|text| is_generated(&text)) {
            fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            *removed += 1;
        }
    }
    Ok(())
}

/// Build a target's module-mapping config from its manifest entry. The manifest format says
/// the most specific remap wins, so the table is ordered longest-prefix-first
/// before the engine's first-match rule sees it. `package` becomes the Go module
/// path; other targets ignore it.
pub(crate) fn codegen_config_for(target: &manifest::ResolvedTarget) -> CodegenConfig {
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

/// The IR to generate from, in precedence order: an explicit path argument,
/// then piped input, then the project's own `.tono` sources compiled through
/// the frontend. The IR is an internal artifact, so a plain `tono gen` inside a
/// project generates straight from source; reading a path or a pipe stays
/// supported for callers that already hold IR (a committed artifact, a CI step
/// that compiled it once, `tono-frontend compile-dir | tono gen`). `tono split`
/// reads its IR through the same contract.
pub(crate) fn read_ir(ir_path: &Option<String>, source_root: &Path) -> Result<Model, String> {
    if let Some(path) = ir_path {
        let json = fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        return decode_model(&json);
    }
    // A terminal is never read: that would hang waiting for input the user has
    // no intention of typing. Anything else may be a pipe, so it is consumed,
    // but blank input is not IR (a script redirecting from /dev/null, a stage
    // that produced nothing), and falling through to the sources beats failing
    // to decode an empty document.
    if !std::io::stdin().is_terminal() {
        let piped = read_stdin()?;
        if !piped.trim().is_empty() {
            return decode_model(&piped);
        }
    }
    decode_model(&compile_sources(source_root)?)
}

/// Compile a whole project to IR by running the frontend over its sources. The
/// frontend owns parsing and typechecking, so its diagnostics are surfaced
/// as-is; a missing binary is an environment gap and says how to point at one.
pub(crate) fn compile_sources(root: &Path) -> Result<String, String> {
    eprintln!("compiling {}", root.display());
    Frontend::from_env().compile_dir(root).map_err(|e| match e {
        FrontendError::Unavailable { program } => {
            format!("could not run {program}; set TONO_FRONTEND to the frontend binary")
        }
        FrontendError::Diagnostics(diagnostics) => diagnostics,
    })
}

/// Parse a `--module-remap from=to` value into its `(from, to)` pair.
fn parse_remap(value: &str) -> Result<(String, String), String> {
    value
        .split_once('=')
        .map(|(from, to)| (from.to_string(), to.to_string()))
        .ok_or_else(|| format!("--module-remap expects <from>=<to>, got: {value}"))
}

fn write_file(dest: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

/// Write one generated file, merging instead of replacing where generation owns
/// only part of what is already on disk.
///
/// The TypeScript `package.json` is the one such file. Generation owns its
/// `exports` map (that is what makes a module's barrel resolvable), but the rest
/// of a package manifest, the name, version, `type`, dependencies and scripts,
/// belongs to the project. Replacing it wholesale would discard the manifest
/// `tono init` scaffolds, along with every edit made to it since.
pub(crate) fn write_generated(dest: &Path, target: TargetKind, text: &str) -> Result<(), String> {
    let is_package_manifest = target == TargetKind::TypeScript
        && dest.file_name().is_some_and(|name| name == "package.json");
    if !is_package_manifest || !dest.is_file() {
        return write_file(dest, text);
    }
    let merged = merge_package_json(dest, text)?;
    write_file(dest, &merged)
}

/// Overlay the generated keys onto the manifest already on disk.
///
/// The overlay is shallow by design: a generated key replaces its counterpart
/// whole, so an `exports` map that lost a module loses that entry here too. A
/// deep merge would keep the stale entry, leaving a specifier pointing at a
/// barrel that is no longer emitted.
fn merge_package_json(dest: &Path, generated: &str) -> Result<String, String> {
    let existing = fs::read_to_string(dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    // A manifest that will not parse is never overwritten: replacing it would
    // destroy whatever it holds, so the run stops and says which file to fix.
    let mut merged: serde_json::Map<String, serde_json::Value> = serde_json::from_str(&existing)
        .map_err(|e| {
            format!(
                "{}: not a JSON object ({e}); fix or remove it, \
                 generation will not overwrite a manifest it cannot read",
                dest.display()
            )
        })?;
    let overlay: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(generated).map_err(|e| format!("generated package.json: {e}"))?;
    merged.extend(overlay);
    let mut text =
        serde_json::to_string_pretty(&merged).map_err(|e| format!("{}: {e}", dest.display()))?;
    text.push('\n');
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    // `cat`, `false`, and a nonexistent binary exercise the three formatter
    // outcomes deterministically, the same stand-ins format.rs's own tests use.
    #[test]
    fn a_clean_format_returns_the_formatted_text() {
        let mut warned = std::collections::HashSet::new();
        let out = apply_format(
            &Formatter::new("cat", vec![]),
            Path::new("pkg/file.go"),
            "package pkg\n",
            &mut warned,
        );
        assert_eq!(out, Ok("package pkg\n".to_string()));
        assert!(warned.is_empty());
    }

    #[test]
    fn a_missing_formatter_falls_back_to_rough_text_and_records_the_program() {
        let mut warned = std::collections::HashSet::new();
        let formatter = Formatter::new("tono-no-such-formatter-xyz", vec![]);
        let out = apply_format(&formatter, Path::new("pkg/file.go"), "rough", &mut warned);
        assert_eq!(out, Ok("rough".to_string()));
        assert!(warned.contains("tono-no-such-formatter-xyz"));

        // A second file through the same missing formatter must not re-record
        // (the stderr note is emitted only on first insertion).
        let out = apply_format(&formatter, Path::new("pkg/other.go"), "rough", &mut warned);
        assert_eq!(out, Ok("rough".to_string()));
        assert_eq!(warned.len(), 1);
    }

    #[test]
    fn a_rejected_input_fails_the_run_naming_the_file_and_program() {
        let mut warned = std::collections::HashSet::new();
        let out = apply_format(
            &Formatter::new("false", vec![]),
            Path::new("pkg/file.go"),
            "broken source",
            &mut warned,
        );
        let err = out.expect_err("a rejection must fail generation");
        assert!(err.contains("pkg/file.go"), "error names the file: {err}");
        assert!(err.contains("false"), "error names the program: {err}");
        assert!(err.contains("bug in tono"), "error owns the blame: {err}");
        assert!(warned.is_empty());
    }
}

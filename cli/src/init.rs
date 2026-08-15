//! `tono init`: create the project manifest (and a minimal native manifest
//! per enabled target) so `gen`/`check`/`fmt`/`preview`/`breaking` have
//! something to auto-discover. Idempotent: re-running against an existing
//! `tono.toml` only adds targets that are not already declared, and never
//! rewrites or reorders what is already there.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write as _};
use std::path::{Path, PathBuf};

use tono_backend::codegen::TargetKind;
use tono_backend::config as manifest;

const NO_TARGETS_ERR: &str =
    "no targets specified; pass --target <list> (e.g. --target ts,rust,go)";

/// A `tono.toml` target key: either a real codegen target, or a known
/// placeholder (`python`/`java`) the manifest schema accepts as a disabled
/// stub but the engine cannot generate yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InitTarget {
    Generatable(TargetKind),
    Placeholder(&'static str),
}

impl InitTarget {
    fn parse(name: &str) -> Result<Self, String> {
        match name {
            "rust" => Ok(Self::Generatable(TargetKind::Rust)),
            "go" => Ok(Self::Generatable(TargetKind::Go)),
            "typescript" | "ts" => Ok(Self::Generatable(TargetKind::TypeScript)),
            "python" => Ok(Self::Placeholder("python")),
            "java" => Ok(Self::Placeholder("java")),
            other => Err(format!(
                "unknown target '{other}' (expected one of: go, java, python, rust, typescript)"
            )),
        }
    }

    /// The manifest's `[target.<key>]` name.
    fn key(self) -> &'static str {
        match self {
            Self::Generatable(kind) => kind.dir(),
            Self::Placeholder(name) => name,
        }
    }
}

pub fn run(args: &[String]) -> Result<(), String> {
    let mut targets_csv: Option<String> = None;
    let mut yes = false;
    let mut root_flag: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => targets_csv = Some(crate::flag_value(args, &mut i, "--target")?),
            "--yes" => yes = true,
            "--root" => root_flag = Some(crate::flag_value(args, &mut i, "--root")?),
            flag => return Err(format!("unknown argument: {flag}\n{}", crate::USAGE)),
        }
        i += 1;
    }

    match crate::discover_manifest() {
        Ok(manifest_path) => {
            // `--root` writes `[project].root`, and update mode never rewrites
            // what the manifest already says. Silently dropping the flag would
            // look like it took effect, so say where it did not apply.
            if root_flag.is_some() {
                eprintln!(
                    "--root is ignored: {} already exists and its [project] is left as it is",
                    manifest_path.display()
                );
            }
            run_update(&manifest_path, &targets_csv, yes)
        }
        Err(_) => run_fresh(&targets_csv, yes, root_flag.as_deref()),
    }
}

fn is_interactive(yes: bool) -> bool {
    !yes && io::stdin().is_terminal()
}

/// The requested targets: from `--target` when given (error if it parses to
/// nothing); otherwise a `y/n` prompt per not-yet-declared generatable target
/// when interactive, or a hard error (never a hang) otherwise. `python`/`java`
/// are only reachable through an explicit `--target`, since they are not a
/// real generation target yet.
fn resolve_targets(
    targets_csv: &Option<String>,
    yes: bool,
    already_declared: &BTreeSet<String>,
) -> Result<Vec<InitTarget>, String> {
    if let Some(csv) = targets_csv {
        let parsed = parse_init_targets(csv)?;
        return if parsed.is_empty() {
            Err(NO_TARGETS_ERR.to_string())
        } else {
            Ok(parsed)
        };
    }
    if !is_interactive(yes) {
        return Err(NO_TARGETS_ERR.to_string());
    }
    let mut chosen = Vec::new();
    for kind in [TargetKind::TypeScript, TargetKind::Rust, TargetKind::Go] {
        if already_declared.contains(kind.dir()) {
            continue;
        }
        if prompt_yes_no(&format!("Enable the {} target?", kind_label(kind)), true) {
            chosen.push(InitTarget::Generatable(kind));
        }
    }
    Ok(chosen)
}

fn parse_init_targets(csv: &str) -> Result<Vec<InitTarget>, String> {
    csv.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(InitTarget::parse)
        .collect()
}

fn kind_label(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Rust => "Rust",
        TargetKind::Go => "Go",
        TargetKind::TypeScript => "TypeScript",
    }
}

// --- update mode -------------------------------------------------------

/// Add any requested targets not already declared to an existing manifest,
/// leaving everything else in the file untouched, and make sure every
/// requested enabled target has its native build manifest on disk.
fn run_update(manifest_path: &Path, targets_csv: &Option<String>, yes: bool) -> Result<(), String> {
    let text = fs::read_to_string(manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let declared = manifest::declared_target_keys(&text)?;
    let cfg = manifest::Config::load(manifest_path)?;
    let targets = resolve_targets(targets_csv, yes, &declared)?;

    let base = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    // A project can gain `ext` blocks with no new target requested at all, so
    // this is scanned regardless of whether `targets` turns out empty below.
    let declared_ext = manifest::declared_ext_keys(&text)?;
    let source_root = base.join(&cfg.project.root);
    let mut additions = String::new();
    for (name, langs) in crate::init_ext::scan_ext_names(&source_root) {
        if !declared_ext.contains(&name) {
            additions.push_str(&crate::init_ext::ext_block(&name, &langs));
        }
    }

    if targets.is_empty() {
        if additions.is_empty() {
            eprintln!("{}: nothing to add", manifest_path.display());
            return Ok(());
        }
        fs::write(manifest_path, append_blocks(&text, &additions))
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
        return Ok(());
    }

    let package_default = cfg
        .project
        .name
        .as_deref()
        .map(slugify)
        .unwrap_or_else(|| slugify(&project_name_default(&base)));

    for target in targets {
        let key = target.key();
        let is_new = !declared.contains(key);
        if is_new {
            additions.push_str(&target_block(target, &package_default));
            eprintln!("{key}: added to {}", manifest_path.display());
        } else {
            report_declared(&cfg, manifest_path, target);
        }
        // Scaffold either way: a target already in the manifest may still be
        // missing its build setup (a hand-written config, or one whose `out`
        // moved), and adopting `init` on such a project is exactly the case
        // that needs it. Scaffolding is a no-op when the file is already there.
        if let InitTarget::Generatable(kind) = target {
            if let Some((out, package)) = scaffold_site(&cfg, kind, is_new, &package_default) {
                scaffold_native_manifest(kind, &base.join(out), &package)?;
            }
        }
    }

    if !additions.is_empty() {
        fs::write(manifest_path, append_blocks(&text, &additions))
            .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    }
    scaffold_gitignore(&base)?;
    Ok(())
}

/// Where a target's native manifest belongs and what package name it carries.
/// A freshly added target takes the values the block just written declares; an
/// already-declared one takes whatever the manifest resolves for it, so the
/// generated `go.mod`/`Cargo.toml` can never contradict the config. `None` for
/// a target that is declared but switched off: `init` does not build out
/// something the project deliberately disabled.
fn scaffold_site(
    cfg: &manifest::Config,
    kind: TargetKind,
    is_new: bool,
    package_default: &str,
) -> Option<(PathBuf, String)> {
    if is_new {
        return Some((
            PathBuf::from("dist").join(kind.dir()),
            default_package(kind, package_default),
        ));
    }
    let resolved = cfg.targets.iter().find(|t| t.kind == kind)?;
    let package = resolved
        .package
        .clone()
        .unwrap_or_else(|| default_package(kind, package_default));
    Some((resolved.out.clone(), package))
}

/// Explain what a requested target that is already in the manifest is doing
/// there, so "nothing happened" is never the whole message.
fn report_declared(cfg: &manifest::Config, manifest_path: &Path, target: InitTarget) {
    let key = target.key();
    let path = manifest_path.display();
    match target {
        // `Config` resolves only enabled, generatable targets, so presence in
        // the resolved list is what "on" means.
        InitTarget::Generatable(kind) if cfg.targets.iter().any(|t| t.kind == kind) => {
            eprintln!("{key}: already enabled in {path}")
        }
        InitTarget::Generatable(_) => eprintln!(
            "{key}: declared in {path} but disabled; \
             set `enabled = true` under [target.{key}] to turn it on"
        ),
        // An enabled placeholder fails `Config::load` outright, so one that
        // reached here is necessarily still switched off.
        InitTarget::Placeholder(_) => eprintln!(
            "{key}: declared in {path} as a placeholder; not generatable yet, leaving it disabled"
        ),
    }
}

/// Append `additions` (one or more `target_block`s, each already leading
/// with its own blank-line separator) after normalizing the existing text's
/// tail: CRLF folded to LF, and exactly one trailing newline so the new
/// block never glues onto the last existing line.
fn append_blocks(existing: &str, additions: &str) -> String {
    let mut text = existing.replace("\r\n", "\n");
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(additions);
    text
}

// --- fresh mode ----------------------------------------------------------

/// Write a new manifest (and per-target native manifests) at the working
/// directory.
fn run_fresh(
    targets_csv: &Option<String>,
    yes: bool,
    root_flag: Option<&str>,
) -> Result<(), String> {
    let cwd = env::current_dir().map_err(|e| e.to_string())?;
    let manifest_path = cwd.join(crate::MANIFEST_NAME);
    let interactive = is_interactive(yes);

    let targets = resolve_targets(targets_csv, yes, &BTreeSet::new())?;
    // A manifest with no target generates nothing, and `tono gen` would only
    // fail on it later; stop here, where the reason is still in view.
    if targets.is_empty() {
        return Err("no targets selected; a manifest with no target generates nothing".to_string());
    }

    let scanned_root = detect_root(&cwd);
    let root = if let Some(r) = root_flag {
        Some(r.to_string())
    } else if interactive {
        let default = scanned_root.as_deref().unwrap_or(".");
        let answer = prompt_text("Root directory for .tono sources", default);
        if answer == "." {
            None
        } else {
            Some(answer)
        }
    } else {
        scanned_root
    };

    let default_name = project_name_default(&cwd);
    let name = if interactive {
        prompt_text("Project name", &default_name)
    } else {
        default_name
    };
    let package_default = slugify(&name);

    let mut text = render_manifest(&name, root.as_deref(), &targets, &package_default);
    let source_root = match &root {
        Some(r) => cwd.join(r),
        None => cwd.clone(),
    };
    for (name, langs) in crate::init_ext::scan_ext_names(&source_root) {
        text.push_str(&crate::init_ext::ext_block(&name, &langs));
    }
    fs::write(&manifest_path, text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    eprintln!("wrote {}", manifest_path.display());

    for target in targets {
        if let InitTarget::Generatable(kind) = target {
            // The same values the block just rendered declares, so the native
            // manifest and the config agree from the start.
            let package = default_package(kind, &package_default);
            scaffold_native_manifest(kind, &cwd.join("dist").join(kind.dir()), &package)?;
        }
    }
    scaffold_gitignore(&cwd)?;
    Ok(())
}

/// Write a starter `.gitignore` next to the manifest, once: an existing file
/// is never touched, here or anywhere else in tono. Whether `dist/` itself is
/// ignored stays the user's call (committed for subtree mirrors, ignored when
/// every mirror is a snapshot), so the entry ships as a comment.
fn scaffold_gitignore(dir: &Path) -> Result<(), String> {
    let path = dir.join(".gitignore");
    if path.exists() {
        return Ok(());
    }
    fs::write(
        &path,
        "# Native toolchain output.\n\
         node_modules/\n\
         target/\n\
         \n\
         # Generated SDKs land under dist/. Keep it committed when a mirror\n\
         # uses split_mode = \"subtree\"; uncomment to ignore it when mirrors\n\
         # use snapshot mode (the default) or there are no mirrors at all.\n\
         # /dist\n",
    )
    .map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

/// The manifest text for a fresh project: `[project]`/`[compat]` with
/// idiomatic defaults, then one commented `[target.<lang>]` block per chosen
/// target. Hand-written (not serde+toml) because the generated file carries
/// explanatory comments, which a round-tripped serializer cannot emit.
fn render_manifest(
    name: &str,
    root: Option<&str>,
    targets: &[InitTarget],
    package_default: &str,
) -> String {
    let mut out = String::new();
    out.push_str(
        "# tono project manifest (RFC-0016). Auto-discovered by gen/check/breaking\n\
         # by walking up from the working directory; no --config flag needed once\n\
         # this file exists.\n\
         \n\
         [project]\n\
         # Falls back to each target's own `package` when it doesn't set one.\n",
    );
    out.push_str(&format!("name = \"{name}\"\n"));
    if let Some(root) = root {
        out.push_str("# Where .tono sources live, relative to this file.\n");
        out.push_str(&format!("root = \"{root}\"\n"));
    }
    out.push_str(
        "\n[compat]\n\
         # Compare against the last release tag by default; point at a fixed ref\n\
         # (e.g. \"git:v1.2.3\") to pin a baseline instead.\n\
         baseline = \"git:last-tag\"\n\
         # Wire and source breaks fail the build; behavioral changes only warn.\n\
         wire_breaking   = \"error\"\n\
         source_breaking = \"error\"\n\
         behavioral      = \"warn\"\n",
    );
    for &target in targets {
        out.push_str(&target_block(target, package_default));
    }
    out
}

/// One `[target.<key>]` block, commented. Leads with a blank line so it can
/// be pushed straight onto a buffer that already ends with a trailing
/// newline (both `render_manifest` and update-mode appends rely on this).
fn target_block(target: InitTarget, package_default: &str) -> String {
    match target {
        InitTarget::Generatable(kind) => {
            let comment = match kind {
                TargetKind::Go => {
                    "# The Go module path other projects import. \"example.com\" is a\n\
                     # placeholder (reserved for documentation, RFC 2606, so it can never\n\
                     # collide with a real one); replace it with your real path (e.g.\n\
                     # \"github.com/you/project\") before publishing.\n"
                }
                _ => "",
            };
            let package = default_package(kind, package_default);
            format!(
                "\n[target.{name}]\n{comment}enabled = true\npackage = \"{package}\"\nout     = \"dist/{dir}\"\n",
                name = kind.dir(),
                dir = kind.dir(),
            )
        }
        InitTarget::Placeholder(name) => format!(
            "\n[target.{name}]\n\
             # Not generatable yet; kept as a placeholder so enabling it later is a\n\
             # one-line change.\n\
             enabled = false\n"
        ),
    }
}

/// The `package` a target gets when the manifest does not name one. Go spells
/// it as a module path, the others take the project slug as-is. Applied once,
/// here: callers pass the result around verbatim so a path the manifest
/// already spells out in full is never prefixed a second time.
fn default_package(kind: TargetKind, base: &str) -> String {
    match kind {
        TargetKind::Go => format!("example.com/{base}"),
        _ => base.to_string(),
    }
}

// --- native manifest scaffolding ------------------------------------------

/// Write a minimal native manifest for `kind` under `dir`, unless one is
/// already there. Deliberately not a complete, buildable project: generated
/// entry/client code emits its own HTTP transport inline, so no runtime
/// dependency needs wiring in beyond what each target's own manifest already
/// declares.
fn scaffold_native_manifest(kind: TargetKind, dir: &Path, package: &str) -> Result<(), String> {
    let (file_name, contents): (&str, String) = match kind {
        TargetKind::Rust => (
            "Cargo.toml",
            format!(
                "[package]\n\
                 name = \"{package}\"\n\
                 version = \"0.1.0\"\n\
                 edition = \"2021\"\n\
                 \n\
                 [dependencies]\n\
                 serde = {{ version = \"1\", features = [\"derive\"] }}\n\
                 serde_json = \"1\"\n"
            ),
        ),
        TargetKind::Go => (
            "go.mod",
            format!(
                "module {package}\n\
                 \n\
                 go 1.21\n"
            ),
        ),
        TargetKind::TypeScript => (
            "package.json",
            format!("{{\n  \"name\": \"{package}\",\n  \"version\": \"0.1.0\",\n  \"type\": \"module\"\n}}\n"),
        ),
    };

    let path = dir.join(file_name);
    if path.exists() {
        eprintln!("{}: already exists, skipping", path.display());
        return Ok(());
    }
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    fs::write(&path, contents).map_err(|e| format!("{}: {e}", path.display()))?;
    eprintln!("wrote {}", path.display());
    Ok(())
}

// --- root/name detection ---------------------------------------------------

const SKIP_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "vendor"];
const MAX_SCAN_DEPTH: usize = 8;

/// Best-effort `root`: the directory holding `.tono` sources, relative to
/// `cwd`. `None` when nothing is found (an empty project) or matches sit
/// under more than one directory (ambiguous; the manifest's own default of
/// `.` is left in place rather than guessing). `project.root` has no
/// consumer today (RFC-0011's module-qualification work is separate), so
/// this stays a lightweight nicety rather than a hardened, symlink-aware
/// walker: the depth bound alone already guarantees termination.
fn detect_root(cwd: &Path) -> Option<String> {
    let mut found = Vec::new();
    scan_for_tono_files(cwd, 0, &mut found);
    if found.is_empty() {
        return None;
    }
    let mut parents: Vec<PathBuf> = found
        .iter()
        .map(|p| p.parent().unwrap_or(cwd).to_path_buf())
        .collect();
    parents.sort();
    parents.dedup();
    let root = match parents.as_slice() {
        [only] => only.clone(),
        _ => return None,
    };
    let rel = root.strip_prefix(cwd).unwrap_or(&root);
    if rel.as_os_str().is_empty() {
        None
    } else {
        Some(rel.to_string_lossy().into_owned())
    }
}

fn scan_for_tono_files(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_SCAN_DEPTH {
        return;
    }
    // A directory the process cannot read (permissions, a transient race) is
    // simply not scanned, rather than failing `init` over an unrelated path.
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
                continue;
            }
            scan_for_tono_files(&path, depth + 1, found);
        } else if path.extension().is_some_and(|ext| ext == "tono") {
            found.push(path);
        }
    }
}

/// The project name default: the working directory's own name.
fn project_name_default(dir: &Path) -> String {
    let absolute = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    absolute
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "project".to_string())
}

/// Lowercase, `[a-z0-9-]`, no repeated or trailing separators: a reasonable
/// default `package` for any of the three target ecosystems.
fn slugify(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

// --- interactive prompts ---------------------------------------------------

/// Interpret an answer to a yes/no prompt. A blank line takes the default, and
/// so does anything unrecognized: the offered answer is already on screen, and
/// proceeding with it beats interrogating someone who typed a stray character.
fn parse_yes_no(line: &str, default_yes: bool) -> bool {
    match line.trim().to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default_yes,
    }
}

/// Interpret an answer to a text prompt: a blank line takes the default.
fn parse_text(line: &str, default: &str) -> String {
    match line.trim() {
        "" => default.to_string(),
        answer => answer.to_string(),
    }
}

/// Ask, on stderr so a redirected stdout never swallows the question, and read
/// the answer. A stdin that cannot be read is not worth stalling over: the
/// default stands, which is the same thing an empty line means.
fn ask(question: &str) -> String {
    eprint!("{question}");
    let _ = io::stderr().flush();
    let mut line = String::new();
    match io::stdin().read_line(&mut line) {
        Ok(_) => line,
        Err(_) => String::new(),
    }
}

fn prompt_yes_no(question: &str, default_yes: bool) -> bool {
    let suffix = if default_yes { "[Y/n]" } else { "[y/N]" };
    parse_yes_no(&ask(&format!("{question} {suffix} ")), default_yes)
}

fn prompt_text(question: &str, default: &str) -> String {
    parse_text(&ask(&format!("{question} [{default}]: ")), default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_yes_no_falls_back_to_the_offered_answer() {
        assert!(parse_yes_no("y", false));
        assert!(parse_yes_no("YES\n", false));
        assert!(!parse_yes_no(" No ", true));
        assert!(!parse_yes_no("n", true));
        // Blank and unrecognized both take the default rather than re-asking.
        assert!(parse_yes_no("", true));
        assert!(!parse_yes_no("", false));
        assert!(parse_yes_no("maybe", true));
        assert!(!parse_yes_no("maybe", false));
    }

    #[test]
    fn parse_text_takes_the_default_only_when_blank() {
        assert_eq!(parse_text("  api  \n", "fallback"), "api");
        assert_eq!(parse_text("", "fallback"), "fallback");
        assert_eq!(parse_text("   \n", "fallback"), "fallback");
    }

    #[test]
    fn slugify_lowercases_and_collapses_separators() {
        assert_eq!(slugify("My Cool  Project!!"), "my-cool-project");
        assert_eq!(slugify("Acme_Payments"), "acme-payments");
        assert_eq!(slugify("---"), "project");
        assert_eq!(slugify(""), "project");
    }

    #[test]
    fn init_target_parse_accepts_aliases_and_placeholders() {
        assert_eq!(
            InitTarget::parse("ts").unwrap(),
            InitTarget::Generatable(TargetKind::TypeScript)
        );
        assert_eq!(
            InitTarget::parse("python").unwrap(),
            InitTarget::Placeholder("python")
        );
        assert!(InitTarget::parse("cobol").is_err());
    }

    #[test]
    fn target_block_renders_expected_toml_shape() {
        let block = target_block(InitTarget::Generatable(TargetKind::Rust), "acme");
        assert!(block.starts_with('\n'));
        assert!(block.contains("[target.rust]"));
        assert!(block.contains("package = \"acme\""));
    }

    #[test]
    fn target_block_for_go_uses_the_example_com_placeholder() {
        let block = target_block(InitTarget::Generatable(TargetKind::Go), "acme");
        assert!(
            block.contains("package = \"example.com/acme\""),
            "go's package should be an unambiguous placeholder module path: {block}"
        );
    }

    /// A target the manifest already configures keeps its own `out` and
    /// `package`, so a scaffolded `go.mod`/`Cargo.toml` can never contradict
    /// the config it was generated from.
    #[test]
    fn scaffold_site_follows_the_manifest_for_a_declared_target() {
        let cfg = manifest::Config::from_toml_str(
            "[target.go]\nenabled = true\npackage = \"github.com/acme/pay\"\nout = \"sdk/go\"\n",
        )
        .unwrap();
        let (out, package) = scaffold_site(&cfg, TargetKind::Go, false, "fallback").unwrap();
        assert_eq!(out, PathBuf::from("sdk/go"));
        // Verbatim: an already-complete module path must not be prefixed again.
        assert_eq!(package, "github.com/acme/pay");
    }

    #[test]
    fn scaffold_site_defaults_a_declared_target_that_names_no_package() {
        let cfg = manifest::Config::from_toml_str("[target.go]\nenabled = true\n").unwrap();
        let (out, package) = scaffold_site(&cfg, TargetKind::Go, false, "acme").unwrap();
        assert_eq!(out, PathBuf::from("dist/go"));
        assert_eq!(package, "example.com/acme");
    }

    /// A disabled target is left alone: `init` does not build out something
    /// the project deliberately switched off.
    #[test]
    fn scaffold_site_skips_a_disabled_target() {
        let cfg = manifest::Config::from_toml_str("[target.go]\nenabled = false\n").unwrap();
        assert!(scaffold_site(&cfg, TargetKind::Go, false, "acme").is_none());
    }

    #[test]
    fn scaffold_site_for_a_new_target_matches_the_block_it_writes() {
        let cfg = manifest::Config::from_toml_str("").unwrap();
        let (out, package) = scaffold_site(&cfg, TargetKind::Go, true, "acme").unwrap();
        assert_eq!(out, PathBuf::from("dist/go"));
        assert!(
            target_block(InitTarget::Generatable(TargetKind::Go), "acme")
                .contains(&format!("package = \"{package}\""))
        );
    }

    #[test]
    fn append_blocks_adds_exactly_one_blank_line() {
        let existing = "[project]\nname = \"demo\"\n";
        let additions = target_block(InitTarget::Placeholder("python"), "demo");
        let result = append_blocks(existing, &additions);
        assert_eq!(
            result,
            "[project]\nname = \"demo\"\n\n[target.python]\n\
             # Not generatable yet; kept as a placeholder so enabling it later is a\n\
             # one-line change.\n\
             enabled = false\n"
        );
    }

    #[test]
    fn append_blocks_normalizes_a_missing_trailing_newline() {
        let existing = "[project]\nname = \"demo\"";
        let additions = target_block(InitTarget::Placeholder("java"), "demo");
        let result = append_blocks(existing, &additions);
        assert!(result.starts_with("[project]\nname = \"demo\"\n\n[target.java]\n"));
    }
}

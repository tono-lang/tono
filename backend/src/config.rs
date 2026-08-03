//! The project manifest: one `tono.toml` at the root, with global defaults and
//! per-target overrides (RFC-0016 / ADR-0026).
//!
//! Parsing has two layers. The `Raw*` structs mirror the TOML surface verbatim
//! (every field optional, unknown keys rejected) so `serde` reports malformed
//! input with a location. [`resolve`] then folds the raw manifest into a
//! [`Config`] with idiomatic defaults filled in and every value validated, so
//! the rest of the engine consumes a total, already-checked config.
//!
//! Scope: this module owns the whole config *surface* (it parses, validates, and
//! defaults every documented key), but only `enabled`, `out`, and the casing
//! overrides drive behavior today. `package`, `lang_version`, `module_mapping`,
//! `module_remap`, and the `compat` severities are carried through so a project
//! can declare them, but their effect belongs to the module-mapping (RFC-0011)
//! and compat-check (RFC-0012) work and is applied there, not here.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::codegen::{CaseStyle, SymbolKind, TargetKind};

/// A fully resolved manifest: global project + compat settings and the list of
/// enabled, generatable targets in a deterministic order.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub project: Project,
    pub compat: Compat,
    /// Enabled and generatable targets only, ordered Rust, Go, TypeScript.
    pub targets: Vec<ResolvedTarget>,
}

/// Global project settings. `root` anchors the qualified module names (RFC-0011)
/// and defaults to the manifest's own directory (`.`).
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub name: Option<String>,
    pub root: PathBuf,
}

/// Compat-check settings (RFC-0012). Parsed and defaulted here; the checker that
/// consumes them is separate work.
#[derive(Debug, Clone, PartialEq)]
pub struct Compat {
    pub baseline: String,
    pub wire_breaking: Severity,
    pub source_breaking: Severity,
    pub behavioral: Severity,
}

/// How a compat break of a given level is enforced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Break the build.
    Error,
    /// Report but do not fail.
    Warn,
    /// Ignore.
    Off,
}

/// Whether a nested module maps to a nested sub-package/namespace or is flattened
/// into one (RFC-0011). Parsed and carried; the mapping itself is separate work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleMapping {
    Nested,
    Flat,
}

/// A resolved per-target config. `out` is where its files are written; `casing`
/// is the set of per-symbol-kind overrides layered on the language's idiomatic
/// defaults. The remaining fields are carried for downstream consumers.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedTarget {
    pub kind: TargetKind,
    pub out: PathBuf,
    pub package: Option<String>,
    pub lang_version: Option<String>,
    pub module_mapping: ModuleMapping,
    pub module_remap: BTreeMap<String, String>,
    /// Idiomatic-casing overrides, ordered by symbol kind for determinism.
    pub casing: Vec<(SymbolKind, CaseStyle)>,
    /// Read-only mirror repository for this target's generated SDK
    /// (`owner/name` shorthand or a full git URL). Opt-in: `None` means the
    /// target lives in the monorepo only and `tono split` skips it.
    pub split_repo: Option<String>,
    /// How `tono split` moves the SDK into the mirror.
    pub split_mode: SplitMode,
}

/// How a target's mirror is produced. `Snapshot` is the default: it needs no
/// committed generated code and appends plain commits, so it works in any
/// repository. `Subtree` is the opt-in for projects that commit the generated
/// SDK and want the mirror to carry the monorepo's own history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SplitMode {
    /// Generate at split time and append the result as one commit.
    #[default]
    Snapshot,
    /// Project the committed history of the `out` subtree, force-pushed.
    Subtree,
}

impl Config {
    /// Parse and resolve a manifest from TOML text.
    pub fn from_toml_str(text: &str) -> Result<Self, String> {
        let raw: RawManifest =
            toml::from_str(text).map_err(|e| format!("invalid tono.toml: {e}"))?;
        resolve(raw)
    }

    /// Read and resolve the manifest at `path`.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_toml_str(&text)
    }
}

/// The raw `[target.*]` keys a manifest text declares, regardless of their
/// enabled/disabled/generatable status. Lets tooling (`tono init`) ask "is
/// there already a table for this name" without resolving the whole config,
/// while still going through the same parser `Config` does (so an unusual
/// but legal TOML spelling, e.g. `[target."python"]`, is not missed the way
/// an ad hoc text scan would miss it).
pub fn declared_target_keys(text: &str) -> Result<BTreeSet<String>, String> {
    let raw: RawManifest = toml::from_str(text).map_err(|e| format!("invalid tono.toml: {e}"))?;
    Ok(raw.target.keys().cloned().collect())
}

// --- raw TOML surface -------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    #[serde(default)]
    project: RawProject,
    #[serde(default)]
    compat: RawCompat,
    #[serde(default)]
    target: BTreeMap<String, RawTarget>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProject {
    name: Option<String>,
    root: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawCompat {
    baseline: Option<String>,
    wire_breaking: Option<String>,
    source_breaking: Option<String>,
    behavioral: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawTarget {
    enabled: Option<bool>,
    package: Option<String>,
    out: Option<String>,
    lang_version: Option<String>,
    module_mapping: Option<String>,
    #[serde(default)]
    module_remap: BTreeMap<String, String>,
    #[serde(default)]
    casing: BTreeMap<String, String>,
    split_repo: Option<String>,
    split_mode: Option<String>,
}

// --- resolution -------------------------------------------------------------

/// The RFC-0012 default severity for each break level.
const DEFAULT_BASELINE: &str = "git:last-tag";

fn resolve(raw: RawManifest) -> Result<Config, String> {
    let project = Project {
        name: raw.project.name,
        root: PathBuf::from(raw.project.root.unwrap_or_else(|| ".".into())),
    };
    let compat = Compat {
        baseline: raw
            .compat
            .baseline
            .unwrap_or_else(|| DEFAULT_BASELINE.into()),
        wire_breaking: severity(raw.compat.wire_breaking, Severity::Error, "wire_breaking")?,
        source_breaking: severity(
            raw.compat.source_breaking,
            Severity::Error,
            "source_breaking",
        )?,
        behavioral: severity(raw.compat.behavioral, Severity::Warn, "behavioral")?,
    };

    let mut targets = Vec::new();
    for (name, rt) in raw.target {
        let enabled = rt.enabled.unwrap_or(true);
        if !enabled {
            continue;
        }
        let Some(kind) = generatable_kind(&name)? else {
            return Err(format!(
                "target '{name}' is enabled but not supported yet (supported: go, rust, typescript)"
            ));
        };
        targets.push(resolve_target(&name, kind, rt)?);
    }
    // A deterministic engine order regardless of the manifest's key order.
    targets.sort_by_key(|t| kind_order(t.kind));

    Ok(Config {
        project,
        compat,
        targets,
    })
}

/// Map a manifest target key to its generatable kind. `python`/`java` are known
/// (RFC-0016 lists them) but not yet generatable, so they resolve to `None`;
/// truly unknown names are an error.
fn generatable_kind(name: &str) -> Result<Option<TargetKind>, String> {
    match name {
        "rust" => Ok(Some(TargetKind::Rust)),
        "go" => Ok(Some(TargetKind::Go)),
        "typescript" => Ok(Some(TargetKind::TypeScript)),
        "python" | "java" => Ok(None),
        other => Err(format!(
            "unknown target '{other}' (expected one of: go, java, python, rust, typescript)"
        )),
    }
}

fn kind_order(kind: TargetKind) -> u8 {
    match kind {
        TargetKind::Rust => 0,
        TargetKind::Go => 1,
        TargetKind::TypeScript => 2,
    }
}

fn resolve_target(name: &str, kind: TargetKind, rt: RawTarget) -> Result<ResolvedTarget, String> {
    // Generated output defaults under dist/ to mark it as a build artifact,
    // separate from the sources it is derived from.
    let out = rt
        .out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("dist").join(kind.dir()));
    let module_mapping = match rt.module_mapping.as_deref() {
        None | Some("nested") => ModuleMapping::Nested,
        Some("flat") => ModuleMapping::Flat,
        Some(other) => {
            return Err(format!(
                "target '{name}': invalid module_mapping '{other}' (expected: nested, flat)"
            ))
        }
    };
    let mut casing = Vec::new();
    for (key, value) in &rt.casing {
        let sym = casing_symbol(name, key)?;
        let style = case_style(name, key, value)?;
        casing.push((sym, style));
    }
    let split_repo = match rt.split_repo {
        Some(repo) if repo.trim().is_empty() => {
            return Err(format!(
                "target '{name}': split_repo must be an 'owner/name' pair or a git URL"
            ))
        }
        other => other,
    };
    let split_mode = match rt.split_mode.as_deref() {
        None => SplitMode::Snapshot,
        Some(_) if split_repo.is_none() => {
            return Err(format!(
                "target '{name}': split_mode has no effect without split_repo"
            ))
        }
        Some("snapshot") => SplitMode::Snapshot,
        Some("subtree") => SplitMode::Subtree,
        Some(other) => {
            return Err(format!(
                "target '{name}': invalid split_mode '{other}' (expected: snapshot, subtree)"
            ))
        }
    };
    Ok(ResolvedTarget {
        kind,
        out,
        package: rt.package,
        lang_version: rt.lang_version,
        module_mapping,
        module_remap: rt.module_remap,
        casing,
        split_repo,
        split_mode,
    })
}

fn severity(raw: Option<String>, default: Severity, field: &str) -> Result<Severity, String> {
    match raw.as_deref() {
        None => Ok(default),
        Some("error") => Ok(Severity::Error),
        Some("warn") => Ok(Severity::Warn),
        Some("off") => Ok(Severity::Off),
        Some(other) => Err(format!(
            "compat.{field}: invalid severity '{other}' (expected: error, warn, off)"
        )),
    }
}

fn casing_symbol(target: &str, key: &str) -> Result<SymbolKind, String> {
    match key {
        "types" => Ok(SymbolKind::Type),
        "fields" => Ok(SymbolKind::Field),
        "methods" => Ok(SymbolKind::Method),
        "enum_members" => Ok(SymbolKind::EnumMember),
        "variants" => Ok(SymbolKind::Variant),
        "modules" => Ok(SymbolKind::Module),
        other => Err(format!(
            "target '{target}': unknown casing key '{other}' \
             (expected: types, fields, methods, enum_members, variants, modules)"
        )),
    }
}

fn case_style(target: &str, key: &str, value: &str) -> Result<CaseStyle, String> {
    match value {
        "pascal" => Ok(CaseStyle::Pascal),
        "camel" => Ok(CaseStyle::Camel),
        "snake" => Ok(CaseStyle::Snake),
        "screaming_snake" => Ok(CaseStyle::ScreamingSnake),
        other => Err(format!(
            "target '{target}': invalid casing '{other}' for '{key}' \
             (expected: pascal, camel, snake, screaming_snake)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The RFC-0016 example, trimmed to the three generatable targets, parses
    /// with every field mapped through.
    const FULL: &str = r#"
[project]
name = "payments-api"
root = "src"

[compat]
baseline = "git:last-tag"
wire_breaking   = "error"
source_breaking = "error"
behavioral      = "warn"

[target.typescript]
enabled      = true
package      = "@acme/payments"
out          = "dist/ts"
lang_version = "ES2022"
module_mapping = "nested"

[target.typescript.casing]
fields = "snake"

[target.typescript.module_remap]
"payments" = "billing"
"payments.charge" = "billing.charges"

[target.rust]
enabled = true
package = "acme-payments"
out     = "dist/rust"

[target.go]
enabled = true
package = "github.com/acme/payments"
out     = "dist/go"

[target.python]
enabled = false

[target.java]
enabled = false
"#;

    #[test]
    fn full_manifest_resolves_every_field() {
        let cfg = Config::from_toml_str(FULL).unwrap();
        assert_eq!(cfg.project.name.as_deref(), Some("payments-api"));
        assert_eq!(cfg.project.root, PathBuf::from("src"));
        assert_eq!(cfg.compat.baseline, "git:last-tag");
        assert_eq!(cfg.compat.wire_breaking, Severity::Error);
        assert_eq!(cfg.compat.behavioral, Severity::Warn);

        // Disabled python/java drop out; the rest are ordered Rust, Go, TS.
        let kinds: Vec<_> = cfg.targets.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            vec![TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript]
        );

        let ts = cfg
            .targets
            .iter()
            .find(|t| t.kind == TargetKind::TypeScript)
            .unwrap();
        assert_eq!(ts.out, PathBuf::from("dist/ts"));
        assert_eq!(ts.package.as_deref(), Some("@acme/payments"));
        assert_eq!(ts.lang_version.as_deref(), Some("ES2022"));
        assert_eq!(ts.module_mapping, ModuleMapping::Nested);
        assert_eq!(ts.casing, vec![(SymbolKind::Field, CaseStyle::Snake)]);
        assert_eq!(
            ts.module_remap.get("payments.charge").map(String::as_str),
            Some("billing.charges")
        );
    }

    #[test]
    fn defaults_apply_when_keys_are_omitted() {
        let cfg = Config::from_toml_str("[target.rust]\n").unwrap();
        // No [project]/[compat]: idiomatic defaults.
        assert_eq!(cfg.project.name, None);
        assert_eq!(cfg.project.root, PathBuf::from("."));
        assert_eq!(cfg.compat.baseline, DEFAULT_BASELINE);
        assert_eq!(cfg.compat.wire_breaking, Severity::Error);
        assert_eq!(cfg.compat.source_breaking, Severity::Error);
        assert_eq!(cfg.compat.behavioral, Severity::Warn);
        // A target table with no fields defaults to enabled, out=dist/<lang>, nested.
        let rust = &cfg.targets[0];
        assert_eq!(rust.kind, TargetKind::Rust);
        assert_eq!(rust.out, PathBuf::from("dist/rust"));
        assert_eq!(rust.module_mapping, ModuleMapping::Nested);
        assert!(rust.casing.is_empty());
        assert_eq!(rust.package, None);
    }

    #[test]
    fn empty_manifest_has_no_targets() {
        let cfg = Config::from_toml_str("").unwrap();
        assert!(cfg.targets.is_empty());
    }

    #[test]
    fn disabled_target_is_dropped() {
        let cfg = Config::from_toml_str("[target.rust]\nenabled = false\n").unwrap();
        assert!(cfg.targets.is_empty());
    }

    #[test]
    fn enabling_an_unsupported_target_is_an_error() {
        let err = Config::from_toml_str("[target.python]\nenabled = true\n").unwrap_err();
        assert!(err.contains("python"), "{err}");
        assert!(err.contains("not supported"), "{err}");
    }

    #[test]
    fn a_disabled_unsupported_target_is_fine() {
        // The RFC example ships python/java as disabled placeholders.
        let cfg = Config::from_toml_str("[target.java]\nenabled = false\n").unwrap();
        assert!(cfg.targets.is_empty());
    }

    #[test]
    fn unknown_target_is_an_error() {
        let err = Config::from_toml_str("[target.cobol]\nenabled = true\n").unwrap_err();
        assert!(err.contains("cobol"), "{err}");
    }

    #[test]
    fn load_reports_a_missing_file() {
        let err = Config::load(Path::new("/no/such/dir/tono.toml")).unwrap_err();
        assert!(err.contains("tono.toml"), "{err}");
    }

    #[test]
    fn unknown_casing_key_is_an_error() {
        let src = "[target.rust.casing]\nwidgets = \"snake\"\n";
        let err = Config::from_toml_str(src).unwrap_err();
        assert!(err.contains("widgets"), "{err}");
        assert!(err.contains("unknown casing key"), "{err}");
    }

    #[test]
    fn invalid_casing_value_is_an_error() {
        let src = "[target.rust.casing]\nfields = \"kebab\"\n";
        let err = Config::from_toml_str(src).unwrap_err();
        assert!(err.contains("kebab"), "{err}");
    }

    #[test]
    fn invalid_module_mapping_is_an_error() {
        let src = "[target.rust]\nmodule_mapping = \"deep\"\n";
        let err = Config::from_toml_str(src).unwrap_err();
        assert!(err.contains("deep"), "{err}");
        assert!(err.contains("module_mapping"), "{err}");
    }

    #[test]
    fn invalid_severity_is_an_error() {
        let src = "[compat]\nwire_breaking = \"fatal\"\n";
        let err = Config::from_toml_str(src).unwrap_err();
        assert!(err.contains("fatal"), "{err}");
        assert!(err.contains("wire_breaking"), "{err}");
    }

    #[test]
    fn malformed_toml_is_an_error() {
        let err = Config::from_toml_str("this is not = = toml").unwrap_err();
        assert!(err.contains("invalid tono.toml"), "{err}");
    }

    #[test]
    fn unknown_top_level_key_is_rejected() {
        let err = Config::from_toml_str("[bogus]\nx = 1\n").unwrap_err();
        assert!(err.contains("invalid tono.toml"), "{err}");
    }

    #[test]
    fn all_severity_levels_parse() {
        let src =
            "[compat]\nwire_breaking=\"off\"\nsource_breaking=\"warn\"\nbehavioral=\"error\"\n";
        let cfg = Config::from_toml_str(src).unwrap();
        assert_eq!(cfg.compat.wire_breaking, Severity::Off);
        assert_eq!(cfg.compat.source_breaking, Severity::Warn);
        assert_eq!(cfg.compat.behavioral, Severity::Error);
    }

    #[test]
    fn declared_target_keys_lists_plain_and_quoted_headers() {
        let keys = declared_target_keys("[target.rust]\n[target.\"python\"]\n").unwrap();
        assert!(keys.contains("rust"));
        assert!(keys.contains("python"));
    }

    #[test]
    fn declared_target_keys_is_empty_for_a_manifest_with_no_targets() {
        let keys = declared_target_keys("[project]\nname = \"demo\"\n").unwrap();
        assert!(keys.is_empty());
    }

    #[test]
    fn declared_target_keys_reports_a_malformed_manifest() {
        let err = declared_target_keys("this is not = = toml").unwrap_err();
        assert!(err.contains("invalid tono.toml"), "{err}");
    }

    #[test]
    fn split_repo_is_carried_through() {
        let src = "[target.rust]\nsplit_repo = \"acme/payments-rust\"\n";
        let cfg = Config::from_toml_str(src).unwrap();
        assert_eq!(
            cfg.targets[0].split_repo.as_deref(),
            Some("acme/payments-rust")
        );
    }

    #[test]
    fn split_repo_defaults_to_none() {
        let cfg = Config::from_toml_str("[target.rust]\n").unwrap();
        assert_eq!(cfg.targets[0].split_repo, None);
    }

    #[test]
    fn blank_split_repo_is_an_error() {
        let err = Config::from_toml_str("[target.rust]\nsplit_repo = \"  \"\n").unwrap_err();
        assert!(err.contains("split_repo"), "{err}");
    }

    #[test]
    fn split_mode_defaults_to_snapshot() {
        let src = "[target.rust]\nsplit_repo = \"acme/sdk\"\n";
        let cfg = Config::from_toml_str(src).unwrap();
        assert_eq!(cfg.targets[0].split_mode, SplitMode::Snapshot);
    }

    #[test]
    fn split_mode_subtree_is_the_opt_in() {
        let src = "[target.rust]\nsplit_repo = \"acme/sdk\"\nsplit_mode = \"subtree\"\n";
        let cfg = Config::from_toml_str(src).unwrap();
        assert_eq!(cfg.targets[0].split_mode, SplitMode::Subtree);
    }

    #[test]
    fn invalid_split_mode_is_an_error() {
        let src = "[target.rust]\nsplit_repo = \"acme/sdk\"\nsplit_mode = \"rsync\"\n";
        let err = Config::from_toml_str(src).unwrap_err();
        assert!(err.contains("rsync"), "{err}");
        assert!(err.contains("split_mode"), "{err}");
    }

    #[test]
    fn split_mode_without_split_repo_is_an_error() {
        let err = Config::from_toml_str("[target.rust]\nsplit_mode = \"snapshot\"\n").unwrap_err();
        assert!(err.contains("split_mode"), "{err}");
        assert!(err.contains("split_repo"), "{err}");
    }

    #[test]
    fn every_casing_key_and_style_maps() {
        let src = "[target.rust.casing]\n\
            types = \"pascal\"\n\
            fields = \"camel\"\n\
            methods = \"snake\"\n\
            enum_members = \"screaming_snake\"\n\
            variants = \"pascal\"\n\
            modules = \"snake\"\n";
        let cfg = Config::from_toml_str(src).unwrap();
        let casing = &cfg.targets[0].casing;
        assert!(casing.contains(&(SymbolKind::Type, CaseStyle::Pascal)));
        assert!(casing.contains(&(SymbolKind::Field, CaseStyle::Camel)));
        assert!(casing.contains(&(SymbolKind::Method, CaseStyle::Snake)));
        assert!(casing.contains(&(SymbolKind::EnumMember, CaseStyle::ScreamingSnake)));
        assert!(casing.contains(&(SymbolKind::Variant, CaseStyle::Pascal)));
        assert!(casing.contains(&(SymbolKind::Module, CaseStyle::Snake)));
    }
}

//! Ext dependency versions (RFC-0023): the pinned `[ext.<name>]` version each
//! enabled target needs, and merging that dependency into the native manifest
//! (`go.mod`, `Cargo.toml`, `package.json`) `gen` writes into.
//!
//! `go.mod`/`Cargo.toml` are not generated files (`tono init` scaffolds them
//! once, and everything past that is the project's own), so this never
//! replaces them wholesale: it patches only the one dependency line an `ext`
//! needs, the same "generation owns only this key" discipline `gen.rs`
//! already applies to `package.json`'s `exports`.

use std::collections::BTreeMap;

use tono_backend::codegen::TargetKind;
use tono_backend::config::{self as manifest, normalize_ext_lang};
use tono_backend::ir::Model;

/// The ext-lang key (as `[ext.<name>]`/`LangPath.lang` spells it) a target
/// kind draws its pinned version from.
fn ext_lang_for(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Rust => "rust",
        TargetKind::Go => "go",
        TargetKind::TypeScript => "ts",
    }
}

/// The `(import path, pinned version)` pairs `kind` needs, deduplicated by
/// path across every module. An `ext` block declared for `kind`'s language
/// with no matching `[ext.<name>]` entry in the manifest is an error naming
/// both the ext and the target, since an enabled target that would import an
/// unpinned dependency cannot produce a buildable SDK.
pub fn ext_deps_for(
    model: &Model,
    cfg: &manifest::Config,
    kind: TargetKind,
) -> Result<Vec<(String, String)>, String> {
    let target_lang = ext_lang_for(kind);
    let mut deps: BTreeMap<String, String> = BTreeMap::new();
    for module in &model.modules {
        for lib in &module.ext_libs {
            for path in &lib.langs {
                if normalize_ext_lang(&path.lang) != target_lang {
                    continue;
                }
                let version = cfg
                    .ext_versions
                    .get(&lib.name)
                    .and_then(|langs| langs.get(target_lang))
                    .ok_or_else(|| {
                        format!(
                            "ext '{}' has no {target_lang} version pinned in [ext.{}] \
                             (required by enabled target {})",
                            lib.name,
                            lib.name,
                            kind.dir()
                        )
                    })?;
                deps.insert(path.path.clone(), version.clone());
            }
        }
    }
    Ok(deps.into_iter().collect())
}

/// Add a `"dependencies"` object to a generated `package.json` body (the one
/// `reexport::typescript_barrels` renders, carrying only `"exports"` today),
/// so it rides the existing shallow overlay onto whatever is already on
/// disk, the same rule already applied to `"exports"`.
pub fn inject_package_json_dependencies(
    generated: &str,
    deps: &[(String, String)],
) -> Result<String, String> {
    let mut obj: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(generated).map_err(|e| format!("generated package.json: {e}"))?;
    let dependencies: serde_json::Map<String, serde_json::Value> = deps
        .iter()
        .map(|(name, version)| (name.clone(), serde_json::Value::String(version.clone())))
        .collect();
    obj.insert(
        "dependencies".to_string(),
        serde_json::Value::Object(dependencies),
    );
    let mut text =
        serde_json::to_string_pretty(&obj).map_err(|e| format!("generated package.json: {e}"))?;
    text.push('\n');
    Ok(text)
}

/// Merge `deps` into an existing `go.mod`'s `require` lines: an existing
/// `require <path> <version>` line has its version replaced in place; a
/// dependency with no existing line is appended as a new one before the
/// file's trailing content. Everything else (the `module`/`go` directives,
/// comments, hand-added requires) is left byte-for-byte untouched. Only the
/// single-line `require` form is recognized, matching what `tono init`
/// scaffolds; a `require (...)` block a user wrote by hand is left alone and
/// the dependency is appended as a new single-line `require` instead.
pub fn merge_go_mod(existing: &str, deps: &[(String, String)]) -> String {
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let mut pending: Vec<(String, String)> = Vec::new();
    for (path, version) in deps {
        let existing_line = lines.iter().position(|l| {
            l.trim_start()
                .strip_prefix("require ")
                .and_then(|rest| rest.split_whitespace().next())
                == Some(path.as_str())
        });
        match existing_line {
            Some(idx) => lines[idx] = format!("require {path} {version}"),
            None => pending.push((path.clone(), version.clone())),
        }
    }
    let mut text = lines.join("\n");
    if !pending.is_empty() {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        for (path, version) in &pending {
            text.push_str(&format!("require {path} {version}\n"));
        }
        return text;
    }
    if existing.ends_with('\n') {
        text.push('\n');
    }
    text
}

/// Merge `deps` into an existing `Cargo.toml`'s `[dependencies]` table: an
/// existing `<crate> = ...` line has its value replaced with the pinned
/// version as a plain string; a dependency with no existing line is inserted
/// right after the `[dependencies]` header. A `[dependencies]` table is
/// created at the end of the file if none exists. Everything outside that one
/// table is left untouched.
///
/// A plain string value always replaces what was there, even an inline table
/// with extra keys (`features`, `optional`, ...): preserving those without a
/// real TOML editor is not reliable, so generation owns the whole line for a
/// dependency it manages, the same way it owns `package.json`'s `exports`.
pub fn merge_cargo_toml(existing: &str, deps: &[(String, String)]) -> String {
    let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
    let header_idx = lines.iter().position(|l| l.trim() == "[dependencies]");
    let (header_idx, mut lines) = match header_idx {
        Some(idx) => (idx, lines),
        None => {
            if !lines.is_empty() {
                lines.push(String::new());
            }
            lines.push("[dependencies]".to_string());
            (lines.len() - 1, lines)
        }
    };
    // The table body runs until the next `[...]` header or EOF.
    let body_end = lines[header_idx + 1..]
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .map(|rel| header_idx + 1 + rel)
        .unwrap_or(lines.len());

    for (krate, version) in deps {
        let existing_line = lines[header_idx + 1..body_end]
            .iter()
            .position(|l| {
                l.trim_start().starts_with(&format!("{krate} ")) || l.trim_start() == *krate
            })
            .map(|rel| header_idx + 1 + rel);
        match existing_line {
            Some(idx) => lines[idx] = format!("{krate} = \"{version}\""),
            None => {
                lines.insert(header_idx + 1, format!("{krate} = \"{version}\""));
            }
        }
    }
    let mut text = lines.join("\n");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_mod_appends_a_missing_require() {
        let out = merge_go_mod(
            "module example.com/x\n\ngo 1.21\n",
            &[("github.com/company/config".into(), "v1.4.2".into())],
        );
        assert!(out.contains("module example.com/x"));
        assert!(out.contains("go 1.21"));
        assert!(out.contains("require github.com/company/config v1.4.2"));
    }

    #[test]
    fn go_mod_replaces_an_existing_require_version() {
        let out = merge_go_mod(
            "module example.com/x\n\ngo 1.21\nrequire github.com/company/config v1.0.0\n",
            &[("github.com/company/config".into(), "v1.4.2".into())],
        );
        assert!(out.contains("require github.com/company/config v1.4.2"));
        assert!(!out.contains("v1.0.0"));
    }

    #[test]
    fn go_mod_leaves_unrelated_lines_untouched() {
        let existing = "module example.com/x\n\ngo 1.21\nrequire github.com/other/lib v2.0.0\n";
        let out = merge_go_mod(
            existing,
            &[("github.com/company/config".into(), "v1.4.2".into())],
        );
        assert!(out.contains("require github.com/other/lib v2.0.0"));
        assert!(out.contains("require github.com/company/config v1.4.2"));
    }

    #[test]
    fn cargo_toml_inserts_a_missing_dependency() {
        let out = merge_cargo_toml(
            "[package]\nname = \"x\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1\"\n",
            &[("company-config".into(), "1.2.0".into())],
        );
        assert!(out.contains("serde = \"1\""));
        assert!(out.contains("company-config = \"1.2.0\""));
    }

    #[test]
    fn cargo_toml_replaces_an_existing_dependency_version() {
        let out = merge_cargo_toml(
            "[dependencies]\ncompany-config = \"1.0.0\"\n",
            &[("company-config".into(), "1.2.0".into())],
        );
        assert!(out.contains("company-config = \"1.2.0\""));
        assert!(!out.contains("1.0.0"));
    }

    #[test]
    fn cargo_toml_creates_dependencies_table_when_absent() {
        let out = merge_cargo_toml(
            "[package]\nname = \"x\"\n",
            &[("company-config".into(), "1.2.0".into())],
        );
        assert!(out.contains("[dependencies]"));
        assert!(out.contains("company-config = \"1.2.0\""));
    }
}

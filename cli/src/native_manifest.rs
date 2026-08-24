//! The native build manifest of a generated SDK (`Cargo.toml`, `go.mod`,
//! `package.json`): scaffolded when absent and patched with what the emitted
//! source requires, so what `tono gen` writes is a buildable package rather
//! than a bag of sources.
//!
//! The manifest is still the project's file: generation creates it only when
//! nothing is there, owns only the ext dependency pins (`gen_ext`), and adds
//! a missing runtime requirement without rewriting one the project tuned.

use std::fs;
use std::path::Path;

use tono_backend::codegen::{native_manifest as required, TargetKind};
use tono_backend::ir::Model;

use crate::gen_ext;

/// Write a minimal native manifest for `kind` under `dir`, unless one is
/// already there. `package` is the crate/package name, and for Go the module
/// path. `Cargo.toml` carries an empty `[workspace]` table so a `dist/`
/// nested inside some larger Cargo workspace still builds on its own.
pub(crate) fn scaffold(kind: TargetKind, dir: &Path, package: &str) -> Result<(), String> {
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
                 serde_json = \"1\"\n\
                 \n\
                 [workspace]\n"
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

/// Make the manifest at `out_dir` declare what the generated sources need:
/// scaffold it when absent (quietly, unlike `init`'s reported scaffolding),
/// ensure the emitted code's own requirements are present, and merge the
/// pinned ext dependencies. TypeScript needs no work here: its `package.json`
/// is written by generation itself and the ext pins ride that merge.
pub(crate) fn ensure(
    kind: TargetKind,
    out_dir: &Path,
    package: &str,
    model: &Model,
    ext_deps: &[(String, String)],
) -> Result<(), String> {
    if kind == TargetKind::TypeScript {
        return Ok(());
    }
    let file_name = match kind {
        TargetKind::Rust => "Cargo.toml",
        TargetKind::Go => "go.mod",
        TargetKind::TypeScript => unreachable!(),
    };
    let path = out_dir.join(file_name);
    if !path.is_file() {
        scaffold(kind, out_dir, package)?;
    }
    let existing = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let merged = match kind {
        TargetKind::Rust => {
            let ensured =
                gen_ext::ensure_cargo_deps(&existing, &required::rust_dependencies(model));
            let ensured = gen_ext::ensure_cargo_features(&ensured, required::rust_features(model));
            if ext_deps.is_empty() {
                ensured
            } else {
                gen_ext::merge_cargo_toml(&ensured, ext_deps)
            }
        }
        TargetKind::Go => {
            if ext_deps.is_empty() {
                existing.clone()
            } else {
                gen_ext::merge_go_mod(&existing, ext_deps)
            }
        }
        TargetKind::TypeScript => unreachable!(),
    };
    if merged != existing {
        fs::write(&path, merged).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tono_backend::ir::decode_model;

    fn tmpdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tono-native-manifest-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn types_only_model() -> Model {
        decode_model(&format!(
            r#"{{"tono_ir_version":{},"modules":[{{"name":"demo","shapes":[{{"id":"demo#Charge","kind":"structure","params":[],"members":[],"operations":[]}}],"operations":[]}}]}}"#,
            tono_backend::ir::TONO_IR_VERSION
        ))
        .unwrap()
    }

    fn entry_model() -> Model {
        decode_model(&format!(
            r#"{{"tono_ir_version":{},"modules":[{{"name":"demo","shapes":[{{"id":"demo#client","kind":"entry","fields":[],"operations":[]}}],"operations":[]}}]}}"#,
            tono_backend::ir::TONO_IR_VERSION
        ))
        .unwrap()
    }

    #[test]
    fn ensure_scaffolds_a_missing_cargo_manifest_that_declares_the_target() {
        let dir = tmpdir("cargo-scaffold");
        ensure(TargetKind::Rust, &dir, "acme-sdk", &types_only_model(), &[]).unwrap();
        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("name = \"acme-sdk\""), "{cargo}");
        assert!(cargo.contains("serde"), "{cargo}");
        // Standalone even inside a larger workspace.
        assert!(cargo.contains("[workspace]"), "{cargo}");
        // A types-only SDK carries no transport stack.
        assert!(!cargo.contains("reqwest"), "{cargo}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_adds_the_transport_stack_for_an_entry_model() {
        let dir = tmpdir("cargo-transport");
        ensure(TargetKind::Rust, &dir, "acme-sdk", &entry_model(), &[]).unwrap();
        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        assert!(cargo.contains("reqwest"), "{cargo}");
        assert!(cargo.contains("tokio"), "{cargo}");
        assert!(cargo.contains("default = [\"reqwest\"]"), "{cargo}");
        assert!(cargo.contains("reqwest = [\"dep:reqwest\"]"), "{cargo}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_merges_ext_pins_into_an_existing_manifest() {
        let dir = tmpdir("cargo-ext");
        fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"mine\"\nversion = \"2.0.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1.0.200\"\n",
        )
        .unwrap();
        ensure(
            TargetKind::Rust,
            &dir,
            "ignored",
            &types_only_model(),
            &[("companyconfig".to_string(), "0.3.0".to_string())],
        )
        .unwrap();
        let cargo = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        // The project's manifest survives; the pin and the missing runtime
        // requirement land beside it.
        assert!(cargo.contains("name = \"mine\""), "{cargo}");
        assert!(cargo.contains("serde = \"1.0.200\""), "{cargo}");
        assert!(cargo.contains("companyconfig = \"0.3.0\""), "{cargo}");
        assert!(cargo.contains("serde_json"), "{cargo}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_scaffolds_a_missing_go_mod_and_merges_requires() {
        let dir = tmpdir("go-scaffold");
        ensure(
            TargetKind::Go,
            &dir,
            "example.com/acme",
            &types_only_model(),
            &[("company.example/config".to_string(), "v0.3.0".to_string())],
        )
        .unwrap();
        let go_mod = fs::read_to_string(dir.join("go.mod")).unwrap();
        assert!(go_mod.contains("module example.com/acme"), "{go_mod}");
        assert!(
            go_mod.contains("require company.example/config v0.3.0"),
            "{go_mod}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn ensure_is_idempotent() {
        let dir = tmpdir("idempotent");
        ensure(TargetKind::Rust, &dir, "acme-sdk", &entry_model(), &[]).unwrap();
        let first = fs::read_to_string(dir.join("Cargo.toml")).unwrap();
        ensure(TargetKind::Rust, &dir, "acme-sdk", &entry_model(), &[]).unwrap();
        assert_eq!(first, fs::read_to_string(dir.join("Cargo.toml")).unwrap());
        let _ = fs::remove_dir_all(&dir);
    }
}

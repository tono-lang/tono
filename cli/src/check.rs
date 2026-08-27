//! `tono check`: the frontend's own diagnostics, then the foreign bindings
//! checked against the libraries they name.
//!
//! The frontend owns parsing and typechecking and prints its diagnostics
//! itself; a source it rejects ends the check there. A source it accepts
//! that declares `ext` blocks goes one step further: each binding is
//! checked against the real library with the target's own toolchain (see
//! `tono_backend::codegen::verify`), and a divergence is reported at the
//! `.tono` span that declared it, in the same shape as the frontend's own
//! diagnostics. The library is resolved the way the generated SDK resolves
//! it: from that target's output directory in `tono.toml`, or from the
//! directory `--lib-root <lang>=<dir>` names. A binding that names one of
//! the module's own types is checked beside the SDK's type declarations,
//! generated in memory under the manifest's layout for that target and
//! written into the probe's scratch tree, so the user need not have run
//! `tono gen`.
//!
//! `tono gen` is untouched by this: generation stays hermetic, and only the
//! check asks for the toolchain of the targets it checks.

use std::path::{Path, PathBuf};

use tono_backend::codegen::verify::{self, LibRoots, Report, TargetRoot};
use tono_backend::config::{self as manifest, normalize_ext_lang};
use tono_backend::ir::decode_model;

use crate::frontend::{Frontend, FrontendError};
use crate::{flag_value, MANIFEST_NAME, USAGE};

/// The parsed command line.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Args {
    pub path: Option<String>,
    pub lib_roots: Vec<(String, PathBuf)>,
    pub config: Option<String>,
    /// The module name the source compiles under (the frontend's
    /// `--module`), which is the generated package the probe sits in; the
    /// file stem otherwise.
    pub module: Option<String>,
}

pub(crate) fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut out = Args::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--lib-root" => {
                let value = flag_value(args, &mut i, "--lib-root")?;
                let (lang, dir) = value.split_once('=').ok_or_else(|| {
                    format!("--lib-root expects <lang>=<dir>, got: {value}\n{USAGE}")
                })?;
                out.lib_roots
                    .push((normalize_ext_lang(lang).to_string(), PathBuf::from(dir)));
            }
            "--config" => out.config = Some(flag_value(args, &mut i, "--config")?),
            "--module" => out.module = Some(flag_value(args, &mut i, "--module")?),
            flag if flag.starts_with("--") => return Err(format!("unknown flag: {flag}\n{USAGE}")),
            p if out.path.is_none() => out.path = Some(p.to_string()),
            p => return Err(format!("unexpected extra argument: {p}\n{USAGE}")),
        }
        i += 1;
    }
    Ok(out)
}

/// The manifest that governs `source`: `--config`, else the nearest
/// `tono.toml` above the file. `None` when there is none, which leaves
/// every language without a root of its own unchecked.
fn manifest_for(source: &Path, config: Option<&str>) -> Option<PathBuf> {
    if let Some(c) = config {
        return Some(PathBuf::from(c));
    }
    let start = source.parent().unwrap_or(Path::new("."));
    let start = std::fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf());
    start
        .ancestors()
        .map(|d| d.join(MANIFEST_NAME))
        .find(|p| p.is_file())
}

/// Where each language resolves its libraries: an explicit `--lib-root`
/// wins; otherwise the target's output directory from the manifest, which
/// is the tree the generated SDK builds in. A root that does not exist on
/// disk is reported, not used. Either way the target's layout (module
/// mapping, casing) comes from the manifest when it declares the target,
/// so the types the probe compiles beside are the ones `tono gen` writes.
pub(crate) fn resolve_roots(
    source: &Path,
    args: &Args,
    notes: &mut Vec<String>,
) -> Result<LibRoots, String> {
    let mut roots = LibRoots::default();
    let mut set = |lang: &str, root: TargetRoot, notes: &mut Vec<String>| {
        if !root.dir.is_dir() {
            notes.push(format!(
                "{lang} bindings: the library root {} does not exist (run tono gen first, or pass --lib-root {lang}=<dir>)",
                root.dir.display()
            ));
            return;
        }
        match lang {
            "go" => roots.go = Some(root),
            "ts" => roots.ts = Some(root),
            _ => {}
        }
    };
    // The layout of each manifest target, by language, for a root the
    // command line names instead of the manifest's own output directory.
    let mut layouts: Vec<(&str, TargetRoot)> = Vec::new();
    if let Some(path) = manifest_for(source, args.config.as_deref()) {
        let cfg = manifest::Config::load(&path)?;
        let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        for target in &cfg.targets {
            let lang = match target.kind {
                tono_backend::codegen::TargetKind::Go => "go",
                tono_backend::codegen::TargetKind::TypeScript => "ts",
                tono_backend::codegen::TargetKind::Rust => continue,
            };
            let root = TargetRoot {
                dir: base.join(&target.out),
                config: crate::gen::codegen_config_for(target),
                casing: target.casing.clone(),
            };
            if args.lib_roots.iter().any(|(l, _)| l == lang) {
                layouts.push((lang, root));
                continue;
            }
            set(lang, root, notes);
        }
    }
    for (lang, dir) in &args.lib_roots {
        match lang.as_str() {
            "go" | "ts" => {
                let layout = layouts
                    .iter()
                    .find(|(l, _)| l == lang)
                    .map(|(_, r)| r.clone())
                    .unwrap_or_default();
                set(
                    lang,
                    TargetRoot {
                        dir: dir.clone(),
                        ..layout
                    },
                    notes,
                )
            }
            other => {
                return Err(format!(
                    "--lib-root: no signature check for {other} (go and ts are checked; rust waits on rustdoc JSON)"
                ))
            }
        }
    }
    Ok(roots)
}

/// Render the report the way the frontend prints diagnostics: findings
/// first, then what was left unchecked, then what passed.
pub(crate) fn render(report: &Report, notes: &[String]) -> String {
    let mut out = String::new();
    for f in &report.findings {
        out.push_str(&f.to_string());
        out.push('\n');
    }
    for line in notes.iter().chain(&report.unchecked) {
        out.push_str("not checked: ");
        out.push_str(line);
        out.push('\n');
    }
    for line in &report.checked {
        out.push_str("checked: ");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn frontend_error(e: FrontendError) -> String {
    match e {
        FrontendError::Unavailable { program } => {
            format!("could not run {program}; set TONO_FRONTEND to the frontend binary")
        }
        FrontendError::Diagnostics(d) => d,
    }
}

/// Run the foreign-binding check on an already frontend-clean source.
/// Returns whether it found no divergence; the report is printed to stderr.
pub(crate) fn check_bindings(source: &Path, args: &Args) -> Result<bool, String> {
    let frontend = Frontend::from_env();
    let ir = frontend
        .compile_as(source, args.module.as_deref())
        .map_err(frontend_error)?;
    let model = decode_model(&ir)?;
    if model.modules.iter().all(|m| m.ext_libs.is_empty()) {
        return Ok(true);
    }
    let sites = verify::parse_sites(&frontend.ext_bindings(source).map_err(frontend_error)?)?;
    let mut notes = Vec::new();
    let roots = resolve_roots(source, args, &mut notes)?;
    let report = verify::verify(&model, &sites, &roots)?;
    eprint!("{}", render(&report, &notes));
    Ok(report.findings.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_path_and_lib_roots() {
        let args: Vec<String> = [
            "a.tono",
            "--lib-root",
            "go=dist/go",
            "--lib-root",
            "typescript=dist/ts",
            "--module",
            "svc",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        let parsed = parse_args(&args).unwrap();
        assert_eq!(parsed.path.as_deref(), Some("a.tono"));
        assert_eq!(parsed.module.as_deref(), Some("svc"));
        assert_eq!(
            parsed.lib_roots,
            vec![
                ("go".to_string(), PathBuf::from("dist/go")),
                ("ts".to_string(), PathBuf::from("dist/ts")),
            ]
        );
    }

    #[test]
    fn rejects_malformed_and_unknown_flags() {
        let bad = |xs: &[&str]| {
            let v: Vec<String> = xs.iter().map(|s| s.to_string()).collect();
            parse_args(&v).unwrap_err()
        };
        assert!(bad(&["a.tono", "--lib-root", "go"]).contains("<lang>=<dir>"));
        assert!(bad(&["a.tono", "--lib-root"]).contains("needs a value"));
        assert!(bad(&["a.tono", "--nope"]).contains("unknown flag"));
        assert!(bad(&["a.tono", "b.tono"]).contains("unexpected extra argument"));
    }

    #[test]
    fn explicit_roots_must_exist_and_name_a_checked_language() {
        let dir = std::env::temp_dir().join(format!("tono-check-roots-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("go")).unwrap();
        let source = dir.join("svc.tono");
        std::fs::write(&source, "").unwrap();
        let args = Args {
            path: None,
            lib_roots: vec![
                ("go".into(), dir.join("go")),
                ("ts".into(), dir.join("missing")),
            ],
            config: None,
            module: None,
        };
        let mut notes = Vec::new();
        let roots = resolve_roots(&source, &args, &mut notes).unwrap();
        assert_eq!(roots.go, Some(TargetRoot::plain(dir.join("go"))));
        assert_eq!(roots.ts, None);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("does not exist"));

        let rust = Args {
            lib_roots: vec![("rust".into(), dir.clone())],
            ..Args::default()
        };
        let err = resolve_roots(&source, &rust, &mut notes).unwrap_err();
        assert!(err.contains("rustdoc JSON"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_roots_come_from_the_targets_out_dirs() {
        let dir = std::env::temp_dir().join(format!("tono-check-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("spec")).unwrap();
        std::fs::create_dir_all(dir.join("out/go")).unwrap();
        std::fs::write(
            dir.join("tono.toml"),
            "[project]\nname = \"demo\"\n[target.go]\nout = \"out/go\"\npackage = \"example.com/demo\"\n[target.typescript]\nout = \"out/ts\"\n[target.rust]\n",
        )
        .unwrap();
        let source = dir.join("spec/svc.tono");
        std::fs::write(&source, "").unwrap();
        let mut notes = Vec::new();
        let roots = resolve_roots(&source, &Args::default(), &mut notes).unwrap();
        assert_eq!(
            roots.go.as_ref().map(|r| r.dir.canonicalize().unwrap()),
            Some(dir.join("out/go").canonicalize().unwrap())
        );
        // The manifest's layout for the target rides along with its root.
        assert_eq!(
            roots.go.as_ref().and_then(|r| r.config.go_module.clone()),
            Some("example.com/demo".to_string())
        );
        assert_eq!(roots.ts, None);
        assert!(
            notes
                .iter()
                .any(|n| n.starts_with("ts bindings") && n.contains("does not exist")),
            "{notes:?}"
        );

        // An explicit root for a language wins over the manifest's directory
        // and keeps the manifest's layout for that target.
        let explicit = Args {
            lib_roots: vec![("go".into(), dir.join("spec"))],
            ..Args::default()
        };
        let roots = resolve_roots(&source, &explicit, &mut Vec::new()).unwrap();
        let go = roots.go.unwrap();
        assert_eq!(go.dir, dir.join("spec"));
        assert_eq!(go.config.go_module.as_deref(), Some("example.com/demo"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_lists_findings_then_unchecked_then_checked() {
        let report = Report {
            findings: vec![verify::Finding {
                span: "3:4-9".into(),
                message: "go binding of op f in ext x: boom".into(),
            }],
            unchecked: vec!["rust bindings of ext x: nightly".into()],
            checked: vec!["go bindings of ext x (go build)".into()],
        };
        let text = render(&report, &["ts bindings: no root".to_string()]);
        assert_eq!(
            text,
            "3:4-9: error: FX0001: go binding of op f in ext x: boom\n\
             not checked: ts bindings: no root\n\
             not checked: rust bindings of ext x: nightly\n\
             checked: go bindings of ext x (go build)\n"
        );
    }
}

use super::*;
use crate::codegen::verify::fixtures::gearbox_module;

fn site(lang: &str, kind: SiteKind, owner: Option<&str>, name: Option<&str>, span: &str) -> Site {
    Site {
        ext: "gearbox".into(),
        lang: lang.into(),
        kind,
        owner: owner.map(str::to_string),
        name: name.map(str::to_string),
        span: span.into(),
    }
}

#[test]
fn sites_parse_one_object_per_line() {
    let text = r#"{"ext":"gearbox","lang":"go","kind":"path","owner":null,"name":null,"span":"2:8-31"}
{"ext":"gearbox","lang":"typescript","kind":"method","owner":"dial","name":"read","span":"15:18-48"}

"#;
    let sites = parse_sites(text).unwrap();
    assert_eq!(
        sites,
        vec![
            site("go", SiteKind::Path, None, None, "2:8-31"),
            site(
                "typescript",
                SiteKind::Method,
                Some("dial"),
                Some("read"),
                "15:18-48"
            ),
        ]
    );
    assert!(parse_sites("{\"ext\":1}")
        .unwrap_err()
        .contains("ext-bindings line"));
}

#[test]
fn site_keys_label_the_binding_as_the_report_names_it() {
    assert_eq!(SiteKey::path().label(), "module path");
    assert_eq!(SiteKey::handle("dial").label(), "handle dial");
    assert_eq!(SiteKey::form("opts").label(), "struct opts");
    assert_eq!(SiteKey::op(None, "open").label(), "op open");
    assert_eq!(
        SiteKey::op(Some("dial"), "read").label(),
        "method dial.read"
    );
    assert_eq!(SiteKey::op(Some("dial"), "read").kind, SiteKind::Method);
    let nameless = SiteKey {
        kind: SiteKind::Handle,
        owner: None,
        name: None,
    };
    assert_eq!(nameless.label(), "handle");
}

#[test]
fn a_probe_numbers_its_lines_and_keeps_its_skips() {
    let mut p = Probe::default();
    p.push_plain("package x");
    p.push(&SiteKey::path(), "import y");
    p.push_plain("");
    p.push(&SiteKey::op(None, "f"), "func f() {}");
    p.skip(&SiteKey::op(None, "g"), "no terms");
    assert_eq!(p.source, "package x\nimport y\n\nfunc f() {}\n");
    assert_eq!(p.lines.get(&2), Some(&SiteKey::path()));
    assert_eq!(p.lines.get(&4), Some(&SiteKey::op(None, "f")));
    assert_eq!(p.skipped, vec!["op g: no terms"]);
}

#[test]
fn a_finding_prints_like_a_frontend_diagnostic() {
    let f = Finding {
        span: "12:5-40".into(),
        message: "go binding of op open in ext gearbox: undefined: gearbox.Open".into(),
    };
    assert_eq!(
        f.to_string(),
        "12:5-40: error: FX0001: go binding of op open in ext gearbox: undefined: gearbox.Open"
    );
}

#[test]
fn go_errors_parse_with_their_continuation_lines() {
    let text = "# probe/.tono-check-go-1\n\
.tono-check-go-1/probe.go:9:127: too many arguments in call to lib.FromConstant[float64]\n\
\thave (float64, float64)\n\
\twant (float64)\n\
.tono-check-go-1/probe.go:11:72: cannot use precision (variable of type uint8) as int value\n\
go: some module problem\n\
other/file.go:3:1: not ours\n";
    let errors = parse_go_errors(text, ".tono-check-go-1/probe.go");
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert_eq!(errors[0].line, Some(9));
    assert_eq!(
        errors[0].message,
        "too many arguments in call to lib.FromConstant[float64]\n\thave (float64, float64)\n\twant (float64)"
    );
    assert_eq!(errors[1].line, Some(11));
    assert_eq!(errors[2].line, None);
    assert_eq!(errors[2].message, "go: some module problem");
    // A continuation with nothing before it is dropped, not a panic.
    assert!(parse_go_errors("\tstray\n", "probe.go").is_empty());
}

#[test]
fn tsc_errors_parse_with_their_continuation_lines() {
    let text = "probe.ts(7,31): error TS2554: Expected 1 arguments, but got 2.\nprobe.ts(5,60): error TS2322: Type 'number' is not assignable to type 'Promise<number>'.\n  Deeper detail here.\nnode_modules/@example/gearbox/index.d.ts(1,1): error TS1000: not ours\ngarbage line\n";
    let errors = parse_tsc_errors(text, "probe.ts");
    assert_eq!(errors.len(), 3, "{errors:?}");
    assert_eq!(errors[0].line, Some(7));
    assert_eq!(
        errors[0].message,
        "error TS2554: Expected 1 arguments, but got 2."
    );
    assert_eq!(errors[1].line, Some(5));
    assert!(errors[1].message.ends_with("\n  Deeper detail here."));
    assert_eq!(errors[2].line, None);
    assert!(parse_tsc_errors("  stray\n", "probe.ts").is_empty());
}

#[test]
fn a_missing_program_is_a_distinct_outcome() {
    let run = ProbeRun {
        program: "tono-no-such-toolchain-xyz".into(),
        args: vec![],
        cwd: std::env::temp_dir(),
        file_label: "probe".into(),
        parse: parse_go_errors,
    };
    assert_eq!(
        run_probe(&run),
        RunOutcome::ToolchainMissing {
            program: "tono-no-such-toolchain-xyz".into()
        }
    );
}

#[test]
fn a_program_that_fails_without_a_position_is_still_reported() {
    let run = ProbeRun {
        program: "sh".into(),
        args: vec!["-c".into(), "echo boom >&2; exit 1".into()],
        cwd: std::env::temp_dir(),
        file_label: "probe".into(),
        parse: parse_go_errors,
    };
    assert_eq!(
        run_probe(&run),
        RunOutcome::Failed(vec![CompilerError {
            line: None,
            message: "boom".into()
        }])
    );
    let ok = ProbeRun {
        program: "sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        cwd: std::env::temp_dir(),
        file_label: "probe".into(),
        parse: parse_go_errors,
    };
    assert_eq!(run_probe(&ok), RunOutcome::Passed);
}

#[test]
fn the_scratch_directory_is_removed_on_drop() {
    let root = std::env::temp_dir().join(format!("tono-scratch-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let dir = {
        let scratch = Scratch::create(&root, "t").unwrap();
        assert!(scratch.dir.is_dir());
        scratch.dir.clone()
    };
    assert!(!dir.exists());
    let _ = std::fs::remove_dir_all(&root);
}

fn probe_with(lines: &[(usize, SiteKey)]) -> Probe {
    let mut p = Probe::default();
    for (n, k) in lines {
        while p.source.lines().count() + 1 < *n {
            p.push_plain("");
        }
        p.push(k, "x");
    }
    p
}

fn sites() -> Vec<Site> {
    vec![
        site("go", SiteKind::Path, None, None, "2:8-31"),
        site("go", SiteKind::Op, None, Some("open"), "21:16-56"),
        site(
            "typescript",
            SiteKind::Method,
            Some("dial"),
            Some("read"),
            "16:18-25",
        ),
    ]
}

#[test]
fn fold_turns_errors_on_binding_lines_into_findings_at_their_span() {
    let probe = probe_with(&[(1, SiteKey::path()), (3, SiteKey::op(None, "open"))]);
    let mut report = Report::default();
    let outcome = RunOutcome::Failed(vec![
        CompilerError {
            line: Some(3),
            message: "undefined: gearbox.Open".into(),
        },
        CompilerError {
            line: Some(2),
            message: "syntax error".into(),
        },
        CompilerError {
            line: None,
            message: "go: weird".into(),
        },
    ]);
    fold_outcome(&mut report, outcome, &probe, &sites(), "gearbox", "go").unwrap();
    assert_eq!(
        report.findings,
        vec![Finding {
            span: "21:16-56".into(),
            message: "go binding of op open in ext gearbox: undefined: gearbox.Open".into()
        }]
    );
    assert_eq!(
        report.unchecked,
        vec![
            "go bindings of ext gearbox: the probe failed outside a binding (syntax error)",
            "go bindings of ext gearbox: the probe failed outside a binding (go: weird)",
        ]
    );
    assert!(report.checked.is_empty());
}

#[test]
fn fold_treats_an_import_error_as_no_type_source_and_drops_the_rest() {
    let mut probe = probe_with(&[(1, SiteKey::path()), (3, SiteKey::op(None, "open"))]);
    probe.skip(&SiteKey::op(None, "stamp"), "moot");
    let mut report = Report::default();
    let outcome = RunOutcome::Failed(vec![
        CompilerError {
            line: Some(1),
            message: "no required module provides package example.test/gearbox; to add it:\n\tgo get example.test/gearbox".into(),
        },
        CompilerError {
            line: Some(3),
            message: "undefined: gearbox".into(),
        },
    ]);
    fold_outcome(&mut report, outcome, &probe, &sites(), "gearbox", "go").unwrap();
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert_eq!(
        report.unchecked,
        vec!["go bindings of ext gearbox: no type source (no required module provides package example.test/gearbox)"]
    );
}

#[test]
fn fold_keeps_skips_and_names_the_tool_on_a_pass() {
    let mut probe = probe_with(&[(1, SiteKey::path())]);
    probe.skip(&SiteKey::op(None, "stamp"), "no terms");
    let mut report = Report::default();
    fold_outcome(
        &mut report,
        RunOutcome::Passed,
        &probe,
        &sites(),
        "gearbox",
        "ts",
    )
    .unwrap();
    assert_eq!(report.checked, vec!["ts bindings of ext gearbox (tsc)"]);
    assert_eq!(
        report.unchecked,
        vec!["ts op stamp: no terms in ext gearbox"]
    );
}

#[test]
fn fold_finds_a_site_under_either_language_spelling_and_falls_back_without_one() {
    let probe = probe_with(&[
        (2, SiteKey::op(Some("dial"), "read")),
        (3, SiteKey::handle("dial")),
    ]);
    let mut report = Report::default();
    let outcome = RunOutcome::Failed(vec![
        CompilerError {
            line: Some(2),
            message: "nope".into(),
        },
        CompilerError {
            line: Some(3),
            message: "nope".into(),
        },
    ]);
    fold_outcome(&mut report, outcome, &probe, &sites(), "gearbox", "ts").unwrap();
    assert_eq!(report.findings[0].span, "16:18-25");
    assert_eq!(report.findings[1].span, "0:0-0");
}

#[test]
fn fold_refuses_a_missing_toolchain() {
    let probe = Probe::default();
    let mut report = Report::default();
    let err = fold_outcome(
        &mut report,
        RunOutcome::ToolchainMissing {
            program: "go".into(),
        },
        &probe,
        &[],
        "gearbox",
        "go",
    )
    .unwrap_err();
    assert!(err.contains("needs go, which is not installed"), "{err}");
}

#[test]
fn verify_reports_every_language_without_a_root_and_rust_always() {
    let model = Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![gearbox_module()],
    };
    let report = verify(&model, &[], &LibRoots::default()).unwrap();
    assert!(report.findings.is_empty());
    assert_eq!(report.unchecked.len(), 3, "{:?}", report.unchecked);
    assert!(report.unchecked[0].starts_with(
        "rust bindings of ext gearbox: reading a crate's signatures needs rustdoc JSON"
    ));
    assert!(report.unchecked[1].contains("--lib-root go=<dir>"));
    assert!(report.unchecked[2].contains("--lib-root ts=<dir>"));
}

#[test]
fn verify_runs_the_probes_where_a_root_is_given() {
    if std::process::Command::new("go")
        .arg("version")
        .output()
        .is_err()
    {
        eprintln!("skipping: go is not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("tono-verify-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("go.mod"), "module consumer\n\ngo 1.21\n").unwrap();
    let model = Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![gearbox_module()],
    };
    let roots = LibRoots {
        go: Some(root.clone()),
        ts: None,
    };
    let report = verify(&model, &[], &roots).unwrap();
    // The stand-in module is not required by the consumer: no type source.
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(
        report
            .unchecked
            .iter()
            .any(|u| u.starts_with("go bindings of ext gearbox: no type source")),
        "{:?}",
        report.unchecked
    );
    let _ = std::fs::remove_dir_all(&root);
}

use super::*;
use crate::codegen::verify::fixtures::gearbox_module;
use crate::codegen::verify::SiteKind;
use crate::ir::Prim;

fn gearbox_probe() -> Probe {
    let m = gearbox_module();
    probe(&m, &m.ext_libs[0])
}

#[test]
fn the_probe_mirrors_what_the_emitter_crosses() {
    let p = gearbox_probe();
    let expected = "\
package tonocheck

import (
\t\"context\"
\tgearbox \"example.test/gearbox\"
)

var _ gearbox.Dial[float64]
func tonoForm_dial_options(tonoForm gearbox.Options) { var _ int = tonoForm.precision; var _ string = tonoForm.label }
func tonoProbe_dial_read(tonoRecv gearbox.Dial[float64], ctx context.Context) { var tonoR0 float64; var tonoR1 error; tonoR0, tonoR1 = tonoRecv.Read(ctx); _, _ = tonoR0, tonoR1 }
func tonoProbe_open(value float64) { var tonoR0 gearbox.Dial[float64]; var tonoR1 error; tonoR0, tonoR1 = gearbox.Open[float64](value); _, _ = tonoR0, tonoR1 }
func tonoProbe_tune(name string, precision uint8) { var tonoR0 gearbox.Dial[float64]; var tonoR1 error; tonoR0, tonoR1 = gearbox.Tune[float64](name, gearbox.WithPrecision(int(precision)), \"fine\"); _, _ = tonoR0, tonoR1 }
func tonoProbe_merge(dials []gearbox.Dial[float64]) { var tonoR0 gearbox.Dial[float64]; var tonoR1 error; tonoR0, tonoR1 = gearbox.Merge[float64](dials...); _, _ = tonoR0, tonoR1 }
func tonoProbe_describe(name string) { var tonoR0 gearbox.Options; var tonoR1 int64; var tonoR2 error; tonoR0, tonoR1, tonoR2 = gearbox.Describe(name); _, _, _ = tonoR0, tonoR1, tonoR2 }
func tonoProbe_raw() { var tonoR0 error; var tonoR1 gearbox.Raw; tonoR0, tonoR1 = gearbox.Raw(); _, _ = tonoR0, tonoR1 }
";
    assert_eq!(p.source, expected);
}

#[test]
fn every_probe_line_maps_to_its_binding() {
    let p = gearbox_probe();
    let at = |n: usize| p.lines.get(&n).expect("mapped line");
    assert_eq!(at(5), &SiteKey::path());
    assert_eq!(at(8), &SiteKey::handle("dial"));
    assert_eq!(at(9), &SiteKey::form("dial_options"));
    assert_eq!(at(10), &SiteKey::op(Some("dial"), "read"));
    assert_eq!(at(11), &SiteKey::op(None, "open"));
    assert_eq!(at(15), &SiteKey::op(None, "raw"));
    assert_eq!(p.lines.len(), 9);
    assert!(p
        .lines
        .values()
        .all(|k| k.kind != SiteKind::Path || k.name.is_none()));
}

#[test]
fn bindings_the_probe_cannot_express_are_listed_with_why() {
    let p = gearbox_probe();
    assert_eq!(
        p.skipped,
        vec![
            "op instantiate: Go has no class reference to pass",
            "op summary: the return type: summary is a type the generated SDK defines",
            "op stamp: parameter at: support.Timestamp is spelled by the generated SDK's support code",
            "op rusty: the struct literal rust_only has no go block",
        ]
    );
}

#[test]
fn context_is_imported_only_when_a_binding_declares_it() {
    let mut m = gearbox_module();
    let lib = &mut m.ext_libs[0];
    lib.types.clear();
    lib.externs.retain(|d| d.name == "open");
    let p = probe(&m, &m.ext_libs[0]);
    assert!(!p.source.contains("\"context\""), "{}", p.source);
    assert_eq!(p.lines.get(&4), Some(&SiteKey::path()));
}

#[test]
fn a_lib_without_a_go_module_has_no_probe() {
    let mut m = gearbox_module();
    m.ext_libs[0].langs.retain(|l| l.lang != "go");
    let p = probe(&m, &m.ext_libs[0]);
    assert_eq!(p, Probe::default());
}

#[test]
fn probe_types_cover_the_declared_vocabulary() {
    let m = gearbox_module();
    let lib = &m.ext_libs[0];
    let t = |t: &Tref| probe_type(&m, lib, t);
    assert_eq!(t(&Tref::Prim(Prim::Bytes)).unwrap(), "[]byte");
    assert_eq!(
        t(&Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::U32))
        ))
        .unwrap(),
        "map[string]uint32"
    );
    assert_eq!(
        t(&Tref::Param("T".into())).unwrap_err(),
        "T is a type parameter"
    );
    let by_lib = Tref::Ref {
        id: "gearbox#dial".into(),
        args: vec![],
    };
    assert_eq!(t(&by_lib).unwrap(), "gearbox.Dial[float64]");
    let no_block = Tref::Ref {
        id: "svc#rust_only".into(),
        args: vec![],
    };
    assert_eq!(
        t(&no_block).unwrap_err(),
        "the struct rust_only declares no go block"
    );
    let mut stripped = gearbox_module();
    stripped.ext_libs[0].types[0].langs.clear();
    assert_eq!(
        probe_type(&stripped, &stripped.ext_libs[0], &by_lib).unwrap_err(),
        "the handle dial declares no go block"
    );
}

#[test]
fn argument_shapes_the_probe_refuses_are_named() {
    let m = gearbox_module();
    let lib = &m.ext_libs[0];
    let err = |a: CallArg| arg_expr(&m, lib, "gearbox", &[], &a).unwrap_err();
    assert_eq!(
        err(CallArg::List(vec![])),
        "an argument shape the probe does not express"
    );
    assert_eq!(
        err(CallArg::ParamAs {
            name: "nope".into(),
            spelling: "int".into()
        }),
        "nope is not a parameter of the op"
    );
    assert_eq!(
        arg_expr(&m, lib, "gearbox", &[], &CallArg::Lit(serde_json::json!(4))).unwrap(),
        "4"
    );
}

#[test]
fn a_yields_position_the_probe_cannot_spell_skips_the_binding() {
    let mut m = gearbox_module();
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "describe")
        .unwrap();
    decl.langs[0].yields[0].r#type = Some(Tref::Ref {
        id: "svc#summary".into(),
        args: vec![],
    });
    let p = probe(&m, &m.ext_libs[0]);
    assert!(p.skipped.contains(
        &"op describe: yields position opts: summary is a type the generated SDK defines"
            .to_string()
    ));
}

#[test]
fn a_handle_without_storage_skips_its_methods() {
    let mut m = gearbox_module();
    m.ext_libs[0].types[0].langs.clear();
    let p = probe(&m, &m.ext_libs[0]);
    assert!(!p.source.contains("tonoProbe_dial_read"));
    assert!(p
        .skipped
        .contains(&"method dial.read: the handle declares no go storage".to_string()));
}

/// The real toolchain, when installed: the probe against a stand-in module
/// that lacks what the declaration names.
#[test]
fn run_reports_the_compiler_line_for_a_wrong_binding() {
    if std::process::Command::new("go")
        .arg("version")
        .output()
        .is_err()
    {
        eprintln!("skipping: go is not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("tono-go-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("gearbox")).unwrap();
    std::fs::write(root.join("go.mod"), "module example.test\n\ngo 1.21\n").unwrap();
    std::fs::write(
        root.join("gearbox/gearbox.go"),
        "package gearbox\n\ntype Dial[T any] interface{ Read() (T, error) }\n\nfunc Open[T any](v T) (Dial[T], error) { return nil, nil }\n",
    )
    .unwrap();
    let mut m = gearbox_module();
    let lib = &mut m.ext_libs[0];
    lib.structs.clear();
    lib.externs.retain(|d| d.name == "open");
    lib.types[0].methods.truncate(1);
    let p = probe(&m, &m.ext_libs[0]);
    let outcome = run(&root, &p).unwrap();
    let RunOutcome::Failed(errors) = outcome else {
        panic!("expected the probe to fail: {outcome:?}");
    };
    // Read takes no context in the stand-in: the method line is the one
    // rejected, and the constructor line passes.
    let line = p
        .lines
        .iter()
        .find(|(_, k)| **k == SiteKey::op(Some("dial"), "read"))
        .map(|(l, _)| *l)
        .unwrap();
    assert!(errors.iter().any(|e| e.line == Some(line)), "{errors:?}");
    assert!(errors.iter().all(|e| e.line == Some(line)), "{errors:?}");
    assert!(
        errors[0].message.contains("too many arguments"),
        "{errors:?}"
    );
    assert!(
        std::fs::read_dir(&root).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tono-check")),
        "the scratch directory is removed"
    );
    let _ = std::fs::remove_dir_all(&root);
}

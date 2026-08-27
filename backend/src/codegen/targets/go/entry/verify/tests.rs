use super::*;
use crate::codegen::verify::fixtures::{gearbox_module, probe_consumer_module, scratch_free};
use crate::codegen::verify::SiteKind;
use crate::ir::{Prim, YieldsPos};

const PRESENT: Sdk = Sdk::Present;

fn gearbox_probe() -> Probe {
    let m = gearbox_module();
    probe(&m, &m.ext_libs[0], &PRESENT, "svc")
}

fn ctx<'a>(m: &'a Module, sdk: &'a Sdk) -> Ctx<'a> {
    Ctx {
        module: m,
        lib: &m.ext_libs[0],
        alias: "gearbox".into(),
        sdk,
    }
}

#[test]
fn the_probe_mirrors_what_the_emitter_crosses() {
    let p = gearbox_probe();
    let expected = "\
package svc

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
func tonoProbe_summary() { var tonoR0 Summary; var tonoR1 error; tonoR0, tonoR1 = gearbox.Summary(); _, _ = tonoR0, tonoR1 }
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
    assert_eq!(at(16), &SiteKey::op(None, "summary"));
    assert_eq!(p.lines.len(), 10);
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
            "op stamp: parameter at: support.Timestamp is spelled by the generated SDK's support code",
            "op rusty: the struct literal rust_only has no go block",
        ]
    );
}

/// Without the generated types beside the probe (the model does not
/// generate), a return the SDK defines cannot be spelled, and the binding
/// is listed with the generation's own reason.
#[test]
fn a_generated_return_without_the_sdk_is_listed_with_the_reason() {
    let m = gearbox_module();
    let absent = Sdk::Absent("no declared test stubs it".into());
    let p = probe(&m, &m.ext_libs[0], &absent, "svc");
    assert!(!p.source.contains("tonoProbe_summary"), "{}", p.source);
    assert!(
        p.skipped.contains(
            &"op summary: the return type: summary, one of the module's own types, needs the generated SDK's types, which are not beside the probe (no declared test stubs it)"
                .to_string()
        ),
        "{:?}",
        p.skipped
    );
}

#[test]
fn context_is_imported_only_when_a_binding_declares_it() {
    let mut m = gearbox_module();
    let lib = &mut m.ext_libs[0];
    lib.types.clear();
    lib.externs.retain(|d| d.name == "open");
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(!p.source.contains("\"context\""), "{}", p.source);
    assert_eq!(p.lines.get(&4), Some(&SiteKey::path()));
}

/// With every binding skipped (no generated types beside the probe, and
/// every line needing them), the library is still imported, blank, so the
/// module path is still resolved and reported on its own line, and neither
/// it nor the context reads as an unused import.
#[test]
fn a_probe_with_every_line_skipped_still_resolves_the_library() {
    let mut m = gearbox_module();
    m.ext_libs[0].types[0].langs[0].name = "Dial[.summary]".into();
    m.ext_libs[0].structs.clear();
    m.ext_libs[0].externs.retain(|d| d.name == "open");
    let open = &mut m.ext_libs[0].externs[0];
    open.langs[0].symbol = "Open[.summary]".into();
    let absent = Sdk::Absent("refused".into());
    let p = probe(&m, &m.ext_libs[0], &absent, "svc");
    assert_eq!(
        p.source,
        "package svc\n\nimport (\n\t_ \"example.test/gearbox\"\n)\n\n"
    );
    assert_eq!(p.lines.get(&4), Some(&SiteKey::path()));
    assert_eq!(p.skipped.len(), 3, "{:?}", p.skipped);
}

#[test]
fn a_lib_without_a_go_module_has_no_probe() {
    let mut m = gearbox_module();
    m.ext_libs[0].langs.retain(|l| l.lang != "go");
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert_eq!(p, Probe::default());
}

#[test]
fn probe_types_cover_the_declared_vocabulary() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let t = |t: &Tref| probe_type(&cx, t);
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
    // The module's own shape is the name the types file declares in this
    // package, a generic one instantiated the Go way.
    let generated = Tref::Ref {
        id: "svc#summary".into(),
        args: vec![],
    };
    assert_eq!(t(&generated).unwrap(), "Summary");
    let generic = Tref::Ref {
        id: "svc#summary".into(),
        args: vec![Tref::Prim(Prim::String), generated.clone()],
    };
    assert_eq!(t(&generic).unwrap(), "Summary[string, Summary]");
    let elsewhere = Tref::Ref {
        id: "other#thing".into(),
        args: vec![],
    };
    assert_eq!(
        t(&elsewhere).unwrap_err(),
        "thing is a type of module other, outside the ext's module"
    );
    let mut stripped = gearbox_module();
    stripped.ext_libs[0].types[0].langs.clear();
    assert_eq!(
        probe_type(&ctx(&stripped, &PRESENT), &by_lib).unwrap_err(),
        "the handle dial declares no go block"
    );
}

/// An entry is construction, not a declaration of the types file: a
/// position typed by one is refused rather than imported from nowhere.
#[test]
fn an_entry_is_not_a_probe_type() {
    let mut m = gearbox_module();
    m.shapes.push(crate::ir::Shape {
        id: "svc#client".into(),
        kind: crate::ir::ShapeKind::Entry {
            fields: vec![],
            operations: vec![],
        },
        traits: vec![],
    });
    let cx = ctx(&m, &PRESENT);
    let entry = Tref::Ref {
        id: "svc#client".into(),
        args: vec![],
    };
    assert_eq!(
        probe_type(&cx, &entry).unwrap_err(),
        "client is an entry, not a type"
    );
    let unknown = Tref::Ref {
        id: "svc#nothing".into(),
        args: vec![],
    };
    assert_eq!(
        probe_type(&cx, &unknown).unwrap_err(),
        "nothing is not a type of module svc"
    );
}

#[test]
fn argument_shapes_the_probe_refuses_are_named() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let err = |a: CallArg| arg_expr(&cx, &[], &a).unwrap_err();
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
        arg_expr(&cx, &[], &CallArg::Lit(serde_json::json!(4))).unwrap(),
        "4"
    );
}

/// A struct literal under a spelling of its own is probed exactly as the
/// emitter passes it (`&gearbox.Options{..}`), while the form itself stays
/// probed as the value type its block declares: the two are graded together
/// by one compile, which is where `#(&Options)` on the block itself failed.
#[test]
fn a_spelled_form_literal_is_probed_by_address() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let literal = |spelling: &str| {
        CallArg::Ctor(crate::ir::CallCtor {
            name: "dial_options".into(),
            fields: std::collections::BTreeMap::from([(
                "label".to_string(),
                CallArg::Lit(serde_json::json!("x")),
            )]),
            spelling: Some(spelling.into()),
        })
    };
    assert_eq!(
        arg_expr(&cx, &[], &literal("&Options")).unwrap(),
        "&gearbox.Options{label: \"x\"}"
    );
    assert_eq!(
        arg_expr(&cx, &[], &literal("Options")).unwrap(),
        "gearbox.Options{label: \"x\"}"
    );
    let err = arg_expr(&cx, &[], &literal("*Options")).unwrap_err();
    assert!(
        err.contains("no conversion from gearbox.Options to *Options"),
        "{err}"
    );
    assert!(
        gearbox_probe()
            .source
            .contains("func tonoForm_dial_options(tonoForm gearbox.Options)"),
        "the form is probed as a value whatever an argument spells"
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
    decl.langs[0].yields[0].r#type = Some(Tref::Prim(Prim::Timestamp));
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(p.skipped.contains(
        &"op describe: yields position opts: support.Timestamp is spelled by the generated SDK's support code"
            .to_string()
    ));
}

#[test]
fn a_handle_without_storage_skips_its_methods() {
    let mut m = gearbox_module();
    m.ext_libs[0].types[0].langs.clear();
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(!p.source.contains("tonoProbe_dial_read"));
    assert!(p
        .skipped
        .contains(&"method dial.read: the handle dial declares no go block".to_string()));
}

fn go_installed() -> bool {
    std::process::Command::new("go")
        .arg("version")
        .output()
        .is_ok()
}

/// A consumer module whose `gearbox` package is `source`, with the model
/// bound to Go only.
fn consumer(name: &str, source: &str) -> (std::path::PathBuf, Module) {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("gearbox")).unwrap();
    std::fs::write(root.join("go.mod"), "module example.test\n\ngo 1.21\n").unwrap();
    std::fs::write(root.join("gearbox/gearbox.go"), source).unwrap();
    (root, probe_consumer_module("go"))
}

/// The real toolchain, when installed: the probe against a stand-in module
/// that lacks what the declaration names.
#[test]
fn run_reports_the_compiler_line_for_a_wrong_binding() {
    if !go_installed() {
        eprintln!("skipping: go is not installed");
        return;
    }
    let (root, m) = consumer(
        "tono-go-probe",
        "package gearbox\n\ntype Dial[T any] interface{ Read() (T, error) }\n\nfunc Open[T any](v T) (Dial[T], error) { return nil, nil }\n",
    );
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    let outcome = {
        let scratch = Scratch::create(&root, "go").unwrap();
        run(&scratch, Path::new("svc"), &p).unwrap()
    };
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
    assert!(scratch_free(&root), "the scratch directory is removed");
    let _ = std::fs::remove_dir_all(&root);
}

/// The real toolchain against the generated types: the handle is generic
/// over the module's own `summary` (`Dial[.summary]`), the types file is
/// generated beside the probe, and a binding wrong only in those terms (the
/// constructor is handed a float where the library wants the Summary the
/// handle is instantiated over) is a finding on the constructor's line,
/// while the handle line, which names the generated type, passes.
#[test]
fn run_grades_a_binding_against_the_generated_types() {
    if !go_installed() {
        eprintln!("skipping: go is not installed");
        return;
    }
    let (root, mut m) = consumer(
        "tono-go-generated",
        "package gearbox\n\nimport \"context\"\n\ntype Dial[T any] interface{ Read(ctx context.Context) (float64, error) }\n\nfunc Open[T any](v T) (Dial[T], error) { return nil, nil }\n",
    );
    m.ext_libs[0].types[0].langs[0].name = "Dial[.summary]".into();
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    open.langs[0].symbol = "Open[.summary]".into();
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![m.clone()],
    };
    let scratch = Scratch::create(&root, "go").unwrap();
    let scratch_name = scratch
        .dir
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let config = crate::codegen::CodegenConfig {
        go_module: Some(format!("example.test/{scratch_name}")),
        ..Default::default()
    };
    let types = crate::codegen::verify::generated_types(
        &model,
        crate::codegen::TargetKind::Go,
        &config,
        &crate::codegen::verify::TargetRoot::default(),
    )
    .unwrap();
    types.write(&scratch.dir).unwrap();
    assert!(scratch.dir.join("svc/types.go").is_file());
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(
        p.source.contains("var _ gearbox.Dial[Summary]"),
        "{}",
        p.source
    );
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    let outcome = run(&scratch, Path::new("svc"), &p).unwrap();
    let RunOutcome::Failed(errors) = outcome else {
        panic!("expected the probe to fail: {outcome:?}");
    };
    let open_line = p
        .lines
        .iter()
        .find(|(_, k)| **k == SiteKey::op(None, "open"))
        .map(|(l, _)| *l)
        .unwrap();
    assert!(
        errors.iter().all(|e| e.line == Some(open_line)),
        "{errors:?}"
    );
    assert!(
        errors[0].message.contains("Summary"),
        "the finding names the generated type: {errors:?}"
    );
    drop(scratch);
    assert!(scratch_free(&root), "the scratch directory is removed");
    let _ = std::fs::remove_dir_all(&root);
}

/// A spelling that references a generated type (`Dial[.summary]`) is
/// written with the type the SDK generates, bare, in the package that
/// declares it; without the generated types beside the probe the handle,
/// its methods and any call spelling it are listed with why.
#[test]
fn a_generated_type_reference_is_probed_against_the_types_or_listed_with_why() {
    let mut m = gearbox_module();
    m.ext_libs[0].types[0].langs[0].name = "Dial[.summary]".into();
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    open.langs[0].symbol = "Open[.summary]".into();
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(
        p.source.contains("var _ gearbox.Dial[Summary]"),
        "{}",
        p.source
    );
    assert!(
        p.source
            .contains("tonoRecv gearbox.Dial[Summary], ctx context.Context"),
        "{}",
        p.source
    );
    assert!(
        p.source.contains("= gearbox.Open[Summary](value)"),
        "{}",
        p.source
    );
    assert!(!p.source.contains(".summary"), "{}", p.source);

    let absent = Sdk::Absent("refused".into());
    let p = probe(&m, &m.ext_libs[0], &absent, "svc");
    assert!(!p.source.contains("var _ gearbox.Dial"), "{}", p.source);
    for why in [
        "handle dial: #(Dial[.summary]), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
        "method dial.read: #(Dial[.summary]), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
        "op open: #(Open[.summary]), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
    ] {
        assert!(p.skipped.contains(&why.to_string()), "{:?}", p.skipped);
    }
}

/// A method chained on the returned object is probed exactly as the emitter
/// writes it, one expression, with the results bound off its last link: the
/// library's own compiler then grades that Read returns something with a
/// Result answering (float64, error).
#[test]
fn a_chained_method_is_probed_off_the_last_link() {
    let mut m = gearbox_module();
    let read = m.ext_libs[0].types[0]
        .methods
        .iter_mut()
        .find(|d| d.name == "read")
        .unwrap();
    let go = read.langs.iter_mut().find(|l| l.lang == "go").unwrap();
    go.chain = Some(crate::ir::SymbolCall {
        symbol: "Result".into(),
        args: vec![CallArg::Lit(serde_json::json!("fast"))],
    });
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(
        p.source.contains(
            "tonoR0, tonoR1 = tonoRecv.Read(ctx).Result(\"fast\"); _, _ = tonoR0, tonoR1"
        ),
        "{}",
        p.source
    );
}

/// A `yields` list that is the call's signature is probed with exactly the
/// declared results: a constructor returning only the handle gets one
/// result and no `error`, the same line `ext::build_call` assigns.
#[test]
fn a_signature_yields_list_is_probed_without_the_implicit_error() {
    let mut m = gearbox_module();
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    decl.langs[0].yields = vec![YieldsPos {
        name: "c".into(),
        r#type: Some(decl.r#return.clone()),
        is_error: false,
        foreign: None,
    }];
    let p = probe(&m, &m.ext_libs[0], &PRESENT, "svc");
    assert!(
        p.source.contains(
            "func tonoProbe_open(value float64) { var tonoR0 gearbox.Dial[float64]; tonoR0 = gearbox.Open[float64](value); _ = tonoR0 }"
        ),
        "{}",
        p.source
    );
}

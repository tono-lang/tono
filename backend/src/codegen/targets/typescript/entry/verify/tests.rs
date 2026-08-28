use super::*;
use crate::codegen::verify::fixtures::{gearbox_module, probe_consumer_module, scratch_free};
use crate::ir::Prim;

const PRESENT: Sdk = Sdk::Present;

fn gearbox_probe() -> Probe {
    let m = gearbox_module();
    probe(&m, &m.ext_libs[0], &PRESENT)
}

fn ctx<'a>(m: &'a Module, sdk: &'a Sdk) -> Ctx<'a> {
    Ctx::new(m, &m.ext_libs[0], sdk)
}

#[test]
fn the_probe_mirrors_what_the_emitter_crosses() {
    let p = gearbox_probe();
    let expected = "\
import * as tonoLib from \"@example/gearbox\";
import { Summary } from \"./types\";

type TonoHandle_dial = tonoLib.Dial<number>;
function tonoForm_dial_options(tonoForm: tonoLib.DialOptions): void { void tonoForm.precision; void tonoForm.label; }
function tonoProbe_dial_read(tonoRecv: TonoHandle_dial): void { const tonoR: Promise<number> = tonoRecv.read(); }
function tonoProbe_dial_label(tonoRecv: TonoHandle_dial, text: string): void { const tonoR: string = tonoRecv.label(text); }
function tonoProbe_open(value: number): void { const tonoR: TonoHandle_dial = new tonoLib.Dial(value); }
function tonoProbe_tune(name: string, precision: number): void { const tonoA0: tonoLib.DialOptions = { label: \"fine\", precision: precision }; const tonoR: TonoHandle_dial = tonoLib.Dial.tune(name, tonoA0); }
function tonoProbe_merge(dials: TonoHandle_dial[]): void { const tonoR: TonoHandle_dial = tonoLib.merge(dials); }
function tonoProbe_describe(name: string): void { const tonoR: tonoLib.RawSummary = tonoLib.describe(name); const tonoV: Summary = tonoR; }
function tonoProbe_instantiate(): void { const tonoR: TonoHandle_dial = tonoLib.instantiate(tonoLib.Dial); }
function tonoProbe_summary(): void { const tonoR: Summary = tonoLib.summary(); }
function tonoProbe_reseed(seed: bigint): void { const tonoR: TonoHandle_dial = tonoLib.reseed(Number(seed)); }
";
    assert_eq!(p.source, expected);
}

#[test]
fn every_probe_line_maps_to_its_binding() {
    let p = gearbox_probe();
    let at = |n: usize| p.lines.get(&n).expect("mapped line");
    assert_eq!(at(1), &SiteKey::path());
    assert_eq!(at(4), &SiteKey::handle("dial"));
    assert_eq!(at(5), &SiteKey::form("dial_options"));
    assert_eq!(at(6), &SiteKey::op(Some("dial"), "read"));
    assert_eq!(at(9), &SiteKey::op(None, "tune"));
    assert_eq!(at(12), &SiteKey::op(None, "instantiate"));
    assert_eq!(at(13), &SiteKey::op(None, "summary"));
    assert_eq!(at(14), &SiteKey::op(None, "reseed"));
    assert_eq!(p.lines.len(), 12);
    assert!(
        !p.lines.contains_key(&2),
        "the types import stands for no binding"
    );
}

/// A probe that names no generated type opens with the library import
/// alone, so nothing is imported from a types file it does not need.
#[test]
fn a_probe_naming_no_generated_type_imports_none() {
    let mut m = gearbox_module();
    m.ext_libs[0].externs.retain(|d| d.name == "open");
    m.ext_libs[0].types[0].methods.clear();
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(!p.source.contains("./types"), "{}", p.source);
    assert_eq!(p.lines.get(&3), Some(&SiteKey::handle("dial")));
}

#[test]
fn bindings_the_probe_cannot_express_are_listed_with_why() {
    let p = gearbox_probe();
    assert_eq!(
        p.skipped,
        vec![
            "op stamp: parameter at: Timestamp is spelled by the generated SDK's support code",
            "op ping: TypeScript binds no position of its own",
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
    let p = probe(&m, &m.ext_libs[0], &absent);
    assert!(!p.source.contains("tonoProbe_summary"), "{}", p.source);
    assert!(!p.source.contains("./types"), "{}", p.source);
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
fn a_lib_without_a_typescript_module_has_no_probe() {
    let mut m = gearbox_module();
    m.ext_libs[0].langs.retain(|l| l.lang != "ts");
    assert_eq!(probe(&m, &m.ext_libs[0], &PRESENT), Probe::default());
    // `typescript` is the other spelling of the same language.
    m.ext_libs[0].langs.push(crate::ir::LangPath {
        lang: "typescript".into(),
        path: "@example/gearbox".into(),
    });
    assert!(probe(&m, &m.ext_libs[0], &PRESENT)
        .source
        .starts_with("import * as tonoLib"));
}

#[test]
fn probe_types_cover_the_declared_vocabulary() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let t = |t: &Tref| probe_type(&cx, t);
    assert_eq!(t(&Tref::Prim(Prim::Bool)).unwrap(), "boolean");
    assert_eq!(
        t(&Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::U32))
        ))
        .unwrap(),
        "Record<string, number>"
    );
    assert_eq!(
        t(&Tref::Param("T".into())).unwrap_err(),
        "T is a type parameter"
    );
    assert!(t(&Tref::Prim(Prim::Duration))
        .unwrap_err()
        .contains("support code"));
    let by_lib = Tref::Ref {
        id: "gearbox#dial".into(),
        args: vec![],
    };
    assert_eq!(t(&by_lib).unwrap(), "TonoHandle_dial");
    let no_block = Tref::Ref {
        id: "svc#rust_only".into(),
        args: vec![],
    };
    assert_eq!(
        t(&no_block).unwrap_err(),
        "the struct rust_only declares no ts block"
    );
    // The module's own shape is the name the types file exports, imported
    // by the probe; a generic one is instantiated the TypeScript way.
    let generated = Tref::Ref {
        id: "svc#summary".into(),
        args: vec![],
    };
    assert_eq!(t(&generated).unwrap(), "Summary");
    let generic = Tref::Ref {
        id: "svc#summary".into(),
        args: vec![Tref::Prim(Prim::String), generated.clone()],
    };
    assert_eq!(t(&generic).unwrap(), "Summary<string, Summary>");
    assert!(cx.generated.borrow().contains("Summary"));
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
        "the handle dial declares no ts block"
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
    assert!(cx.generated.borrow().is_empty());
}

#[test]
fn argument_shapes_the_probe_refuses_are_named() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let mut prelude = Vec::new();
    let err = |a: CallArg, prelude: &mut Vec<String>| arg_expr(&cx, &[], &a, prelude).unwrap_err();
    assert_eq!(
        err(CallArg::Ref(vec!["x".into()]), &mut prelude),
        "an argument shape the probe does not express"
    );
    assert_eq!(
        err(
            CallArg::Ctor(crate::ir::CallCtor {
                name: "rust_only".into(),
                fields: Default::default(),
                spelling: None,
            }),
            &mut prelude
        ),
        "the struct literal rust_only has no ts block"
    );
    let nested = CallArg::SymbolCall(crate::ir::SymbolCall {
        symbol: "pick".into(),
        args: vec![CallArg::Lit(serde_json::json!(2))],
    });
    assert_eq!(
        arg_expr(&cx, &[], &nested, &mut prelude).unwrap(),
        "tonoLib.pick(2)"
    );
    assert!(prelude.is_empty());
}

/// A struct literal under a spelling of its own is probed as the literal
/// the emitter passes (structural), and a primitive spelling is named.
#[test]
fn a_spelled_form_literal_is_probed_as_the_literal() {
    let m = gearbox_module();
    let cx = ctx(&m, &PRESENT);
    let literal = |spelling: &str| {
        CallArg::Ctor(crate::ir::CallCtor {
            name: "dial_options".into(),
            fields: Default::default(),
            spelling: Some(spelling.into()),
        })
    };
    let mut prelude = Vec::new();
    assert_eq!(
        arg_expr(&cx, &[], &literal("Options"), &mut prelude).unwrap(),
        "tonoA0"
    );
    assert_eq!(prelude.len(), 1);
    let err = arg_expr(&cx, &[], &literal("number"), &mut prelude).unwrap_err();
    assert!(
        err.contains("no conversion from DialOptions to number"),
        "{err}"
    );
}

#[test]
fn a_yields_position_without_a_type_or_spelling_is_refused() {
    let mut m = gearbox_module();
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "describe")
        .unwrap();
    decl.langs[1].yields[1].foreign = None;
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(p
        .skipped
        .contains(&"op describe: yields position raw has no type".to_string()));
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "describe")
        .unwrap();
    decl.langs[1].yields[1].r#type = Some(Tref::Prim(Prim::Timestamp));
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(p.skipped.contains(
        &"op describe: yields position raw: Timestamp is spelled by the generated SDK's support code"
            .to_string()
    ));
}

/// A spelled answer is what the library gives; the probe types the result
/// as the spelling and then reads it back into the declared return
/// through the same conversion the emitter writes, so `tsc` grades both
/// steps: the library's own signature and the way back.
#[test]
fn a_spelled_answer_is_probed_as_given_and_converted_back() {
    let mut m = gearbox_module();
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "describe")
        .unwrap();
    decl.r#return = Tref::Map(
        Box::new(Tref::Prim(crate::ir::Prim::String)),
        Box::new(Tref::Prim(crate::ir::Prim::String)),
    );
    decl.langs[1].yields = vec![crate::ir::YieldsPos {
        name: "table".into(),
        r#type: None,
        is_error: false,
        foreign: Some("Map<string, string>".into()),
    }];
    decl.langs[1].returns = None;
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(
        p.source.contains(
            "const tonoR: Map<string, string> = tonoLib.describe(name); const tonoV: Record<string, string> = Object.fromEntries(tonoR);"
        ),
        "{}",
        p.source
    );
}

#[test]
fn a_handle_without_storage_skips_its_methods() {
    let mut m = gearbox_module();
    m.ext_libs[0].types[0].langs.clear();
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(!p.source.contains("tonoProbe_dial_read"));
    assert!(p
        .skipped
        .contains(&"method dial.read: the handle declares no ts storage".to_string()));
}

#[test]
fn tsc_comes_from_the_consumer_tree_when_installed_there() {
    let root = std::env::temp_dir().join(format!("tono-tsc-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let nested = root.join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(tsc_program(&nested), PathBuf::from("tsc"));
    let bin = root.join("node_modules/.bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::write(bin.join("tsc"), "").unwrap();
    assert_eq!(tsc_program(&nested), bin.join("tsc"));
    let _ = std::fs::remove_dir_all(&root);
}

fn tsc_installed() -> bool {
    std::process::Command::new("tsc")
        .arg("--version")
        .output()
        .is_ok()
}

/// A consumer tree whose `@example/gearbox` package declares `dts`, with
/// the model bound to TypeScript only.
fn consumer(name: &str, dts: &str) -> (std::path::PathBuf, Module) {
    let root = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pkg = root.join("node_modules/@example/gearbox");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"index.d.ts\"}",
    )
    .unwrap();
    std::fs::write(pkg.join("index.d.ts"), dts).unwrap();
    (root, probe_consumer_module("ts"))
}

/// The real toolchain, when installed: the probe against a stand-in package
/// whose class takes one argument where the declaration passes two.
#[test]
fn run_reports_the_compiler_line_for_a_wrong_binding() {
    if !tsc_installed() {
        eprintln!("skipping: tsc is not installed");
        return;
    }
    let (root, m) = consumer(
        "tono-ts-probe",
        "export interface Dial<T> { read(): T; }\nexport class Dial<T> { constructor(value: T); }\n",
    );
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    let outcome = {
        let scratch = Scratch::create(&root, "ts").unwrap();
        run(&scratch, Path::new("svc"), &p).unwrap()
    };
    let RunOutcome::Failed(errors) = outcome else {
        panic!("expected the probe to fail: {outcome:?}");
    };
    // read() is synchronous in the stand-in while the op declares it
    // asynchronous for TypeScript: the method line is the one rejected.
    let line = p
        .lines
        .iter()
        .find(|(_, k)| **k == SiteKey::op(Some("dial"), "read"))
        .map(|(l, _)| *l)
        .unwrap();
    assert!(errors.iter().all(|e| e.line == Some(line)), "{errors:?}");
    assert!(errors[0].message.contains("Promise<number>"), "{errors:?}");
    assert!(scratch_free(&root), "the scratch directory is removed");
    let _ = std::fs::remove_dir_all(&root);
}

/// The real toolchain against the generated types: the handle is generic
/// over the module's own `summary` (`Dial<.summary>`), the types file is
/// generated beside the probe, and a binding wrong only in those terms (the
/// library bounds its parameter to `string`, which the generated Summary is
/// not) is a finding on the handle's line, the span the `.tono` wrote the
/// spelling at, while the constructor line, which spells the same type,
/// passes on its own.
#[test]
fn run_grades_a_binding_against_the_generated_types() {
    if !tsc_installed() {
        eprintln!("skipping: tsc is not installed");
        return;
    }
    let (root, mut m) = consumer(
        "tono-ts-generated",
        "export class Dial<T extends string> { constructor(value: T); read(): Promise<number>; }\n",
    );
    let dial = m.ext_libs[0].types[0]
        .langs
        .iter_mut()
        .find(|l| l.lang == "ts")
        .unwrap();
    dial.name = Some("Dial<.summary>".into());
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    open.langs[0].symbol = "new Dial<.summary>".into();
    open.params[0].r#type = Tref::Ref {
        id: "svc#summary".into(),
        args: vec![],
    };
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![m.clone()],
    };
    let scratch = Scratch::create(&root, "ts").unwrap();
    let types = crate::codegen::verify::generated_types(
        &model,
        crate::codegen::TargetKind::TypeScript,
        &Default::default(),
        &crate::codegen::verify::TargetRoot::default(),
    )
    .unwrap();
    types.write(&scratch.dir).unwrap();
    assert!(scratch.dir.join("svc/types.ts").is_file());
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(
        p.source.contains("import { Summary } from \"./types\";"),
        "{}",
        p.source
    );
    assert!(
        p.source
            .contains("type TonoHandle_dial = tonoLib.Dial<Summary>;"),
        "{}",
        p.source
    );
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    let outcome = run(&scratch, Path::new("svc"), &p).unwrap();
    let RunOutcome::Failed(errors) = outcome else {
        panic!("expected the probe to fail: {outcome:?}");
    };
    let handle_line = p
        .lines
        .iter()
        .find(|(_, k)| **k == SiteKey::handle("dial"))
        .map(|(l, _)| *l)
        .unwrap();
    let open_line = p
        .lines
        .iter()
        .find(|(_, k)| **k == SiteKey::op(None, "open"))
        .map(|(l, _)| *l)
        .unwrap();
    assert!(
        errors
            .iter()
            .all(|e| e.line == Some(handle_line) || e.line == Some(open_line)),
        "{errors:?}"
    );
    assert!(
        errors.iter().any(|e| e.line == Some(handle_line)),
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

/// A spelling that references a generated type (`Dial<.summary>`) is
/// written with the type the SDK generates, imported from the types file
/// beside the probe; without the generated types the handle, its methods
/// and any call spelling it are listed with why, and nothing else in the
/// probe is affected.
#[test]
fn a_generated_type_reference_is_probed_against_the_types_or_listed_with_why() {
    let mut m = gearbox_module();
    let dial = m.ext_libs[0].types[0]
        .langs
        .iter_mut()
        .find(|l| l.lang == "ts")
        .unwrap();
    dial.name = Some("Dial<.summary>".into());
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    let open_ts = open.langs.iter_mut().find(|l| l.lang == "ts").unwrap();
    open_ts.symbol = "new Dial<.summary>".into();
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(
        p.source
            .contains("type TonoHandle_dial = tonoLib.Dial<Summary>;"),
        "{}",
        p.source
    );
    assert!(
        p.source.contains("= new tonoLib.Dial<Summary>(value);"),
        "{}",
        p.source
    );
    assert!(
        p.source.contains("import { Summary } from \"./types\";"),
        "{}",
        p.source
    );
    assert!(!p.source.contains("<.summary>"), "{}", p.source);

    let absent = Sdk::Absent("refused".into());
    let p = probe(&m, &m.ext_libs[0], &absent);
    assert!(!p.source.contains("type TonoHandle_dial"), "{}", p.source);
    assert!(p.source.contains("tonoProbe_describe"), "{}", p.source);
    for why in [
        "handle dial: #(Dial<.summary>), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
        "method dial.read: #(Dial<.summary>), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
        "op open: #(new Dial<.summary>), which references .summary, needs the generated SDK's types, which are not beside the probe (refused)",
    ] {
        assert!(p.skipped.contains(&why.to_string()), "{:?}", p.skipped);
    }
}

/// A class reference to one of the module's own structs passes the class
/// the types file declares beside the interface, imported like any
/// generated type; without the types beside the probe it is listed with
/// why.
#[test]
fn a_class_reference_to_a_generated_struct_is_imported_or_listed_with_why() {
    let mut m = gearbox_module();
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    let open_ts = open.langs.iter_mut().find(|l| l.lang == "ts").unwrap();
    open_ts.call_args = vec![crate::ir::CallArg::TypeRef("summary".into())];
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(
        p.source.contains("= new tonoLib.Dial(Summary);"),
        "{}",
        p.source
    );
    assert!(
        p.source.contains("import { Summary } from \"./types\";"),
        "{}",
        p.source
    );
    let absent = Sdk::Absent("refused".into());
    let p = probe(&m, &m.ext_libs[0], &absent);
    assert!(
        p.skipped.contains(
            &"op open: summary, one of the module's own types, needs the generated SDK's types, which are not beside the probe (refused)"
                .to_string()
        ),
        "{:?}",
        p.skipped
    );
}

/// Generation refuses a chained call for TypeScript, so the probe has no
/// expression to mirror: the binding is listed as skipped with why, and
/// the rest of the library is still probed.
#[test]
fn a_chained_call_skips_the_binding_with_why() {
    let mut m = gearbox_module();
    let read = m.ext_libs[0].types[0]
        .methods
        .iter_mut()
        .find(|d| d.name == "read")
        .unwrap();
    let ts = read.langs.iter_mut().find(|l| l.lang == "ts").unwrap();
    ts.chain = Some(crate::ir::SymbolCall {
        symbol: "result".into(),
        args: vec![],
    });
    let p = probe(&m, &m.ext_libs[0], &PRESENT);
    assert!(!p.source.contains("tonoProbe_dial_read"), "{}", p.source);
    assert!(p.source.contains("tonoProbe_open"), "{}", p.source);
    assert!(
        p.skipped.contains(
            &"method dial.read: TypeScript has no chained call to probe (.result(..) on the returned object)"
                .to_string()
        ),
        "{:?}",
        p.skipped
    );
}

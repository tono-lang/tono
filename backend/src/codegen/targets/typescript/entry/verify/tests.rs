use super::*;
use crate::codegen::verify::fixtures::gearbox_module;
use crate::ir::Prim;

fn gearbox_probe() -> Probe {
    let m = gearbox_module();
    probe(&m, &m.ext_libs[0])
}

#[test]
fn the_probe_mirrors_what_the_emitter_crosses() {
    let p = gearbox_probe();
    let expected = "\
import * as tonoLib from \"@example/gearbox\";

type TonoHandle_dial = tonoLib.Dial<number>;
function tonoForm_dial_options(tonoForm: tonoLib.DialOptions): void { void tonoForm.precision; void tonoForm.label; }
function tonoProbe_dial_read(tonoRecv: TonoHandle_dial): void { const tonoR: Promise<number> = tonoRecv.read(); }
function tonoProbe_dial_label(tonoRecv: TonoHandle_dial, text: string): void { const tonoR: string = tonoRecv.label(text); }
function tonoProbe_open(value: number): void { const tonoR: TonoHandle_dial = new tonoLib.Dial(value); }
function tonoProbe_tune(name: string, precision: number): void { const tonoA0: tonoLib.DialOptions = { label: \"fine\", precision: precision }; const tonoR: TonoHandle_dial = tonoLib.Dial.tune(name, tonoA0); }
function tonoProbe_merge(dials: TonoHandle_dial[]): void { const tonoR: TonoHandle_dial = tonoLib.merge(dials); }
function tonoProbe_describe(name: string): void { const tonoR: tonoLib.RawSummary = tonoLib.describe(name); }
function tonoProbe_instantiate(): void { const tonoR: TonoHandle_dial = tonoLib.instantiate(tonoLib.Dial); }
function tonoProbe_reseed(seed: bigint): void { const tonoR: TonoHandle_dial = tonoLib.reseed(Number(seed)); }
";
    assert_eq!(p.source, expected);
}

#[test]
fn every_probe_line_maps_to_its_binding() {
    let p = gearbox_probe();
    let at = |n: usize| p.lines.get(&n).expect("mapped line");
    assert_eq!(at(1), &SiteKey::path());
    assert_eq!(at(3), &SiteKey::handle("dial"));
    assert_eq!(at(4), &SiteKey::form("dial_options"));
    assert_eq!(at(5), &SiteKey::op(Some("dial"), "read"));
    assert_eq!(at(8), &SiteKey::op(None, "tune"));
    assert_eq!(at(11), &SiteKey::op(None, "instantiate"));
    assert_eq!(at(12), &SiteKey::op(None, "reseed"));
    assert_eq!(p.lines.len(), 11);
}

#[test]
fn bindings_the_probe_cannot_express_are_listed_with_why() {
    let p = gearbox_probe();
    assert_eq!(
        p.skipped,
        vec![
            "op summary: the return type: summary is a type the generated SDK defines",
            "op stamp: parameter at: Timestamp is spelled by the generated SDK's support code",
            "op ping: TypeScript binds no position of its own",
        ]
    );
}

#[test]
fn a_lib_without_a_typescript_module_has_no_probe() {
    let mut m = gearbox_module();
    m.ext_libs[0].langs.retain(|l| l.lang != "ts");
    assert_eq!(probe(&m, &m.ext_libs[0]), Probe::default());
    // `typescript` is the other spelling of the same language.
    m.ext_libs[0].langs.push(crate::ir::LangPath {
        lang: "typescript".into(),
        path: "@example/gearbox".into(),
    });
    assert!(probe(&m, &m.ext_libs[0])
        .source
        .starts_with("import * as tonoLib"));
}

#[test]
fn probe_types_cover_the_declared_vocabulary() {
    let m = gearbox_module();
    let lib = &m.ext_libs[0];
    let t = |t: &Tref| probe_type(&m, lib, t);
    assert_eq!(t(&Tref::Prim(Prim::Bool)).unwrap(), "boolean");
    assert_eq!(
        t(&Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::U32))
        ))
        .unwrap(),
        "Map<string, number>"
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
    let mut stripped = gearbox_module();
    stripped.ext_libs[0].types[0].langs.clear();
    assert_eq!(
        probe_type(&stripped, &stripped.ext_libs[0], &by_lib).unwrap_err(),
        "the handle dial declares no ts block"
    );
}

#[test]
fn argument_shapes_the_probe_refuses_are_named() {
    let m = gearbox_module();
    let lib = &m.ext_libs[0];
    let mut prelude = Vec::new();
    let err =
        |a: CallArg, prelude: &mut Vec<String>| arg_expr(&m, lib, &[], &a, prelude).unwrap_err();
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
        arg_expr(&m, lib, &[], &nested, &mut prelude).unwrap(),
        "tonoLib.pick(2)"
    );
    assert!(prelude.is_empty());
}

/// A struct literal under a spelling of its own is probed as the literal
/// the emitter passes (structural), and a primitive spelling is named.
#[test]
fn a_spelled_form_literal_is_probed_as_the_literal() {
    let m = gearbox_module();
    let lib = &m.ext_libs[0];
    let literal = |spelling: &str| {
        CallArg::Ctor(crate::ir::CallCtor {
            name: "dial_options".into(),
            fields: Default::default(),
            spelling: Some(spelling.into()),
        })
    };
    let mut prelude = Vec::new();
    assert_eq!(
        arg_expr(&m, lib, &[], &literal("Options"), &mut prelude).unwrap(),
        "tonoA0"
    );
    assert_eq!(prelude.len(), 1);
    let err = arg_expr(&m, lib, &[], &literal("number"), &mut prelude).unwrap_err();
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
    let p = probe(&m, &m.ext_libs[0]);
    assert!(p
        .skipped
        .contains(&"op describe: yields position raw has no type".to_string()));
    let decl = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "describe")
        .unwrap();
    decl.langs[1].yields[1].r#type = Some(Tref::Ref {
        id: "svc#summary".into(),
        args: vec![],
    });
    let p = probe(&m, &m.ext_libs[0]);
    assert!(p.skipped.contains(
        &"op describe: yields position raw: summary is a type the generated SDK defines"
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

/// The real toolchain, when installed: the probe against a stand-in package
/// whose class takes one argument where the declaration passes two.
#[test]
fn run_reports_the_compiler_line_for_a_wrong_binding() {
    if std::process::Command::new("tsc")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping: tsc is not installed");
        return;
    }
    let root = std::env::temp_dir().join(format!("tono-ts-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let pkg = root.join("node_modules/@example/gearbox");
    std::fs::create_dir_all(&pkg).unwrap();
    std::fs::write(
        pkg.join("package.json"),
        "{\"name\":\"@example/gearbox\",\"version\":\"0.0.0\",\"types\":\"index.d.ts\"}",
    )
    .unwrap();
    std::fs::write(
        pkg.join("index.d.ts"),
        "export interface Dial<T> { read(): T; }\nexport class Dial<T> { constructor(value: T); }\n",
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

/// A spelling that references a generated type (`Dial<.summary>`) cannot
/// be written without the generated SDK: the handle, its methods and any
/// call spelling it are listed as skipped with why, and nothing else in
/// the probe is affected.
#[test]
fn a_generated_type_reference_skips_the_binding_with_why() {
    let mut m = gearbox_module();
    let dial = m.ext_libs[0].types[0]
        .langs
        .iter_mut()
        .find(|l| l.lang == "ts")
        .unwrap();
    dial.name = "Dial<.summary>".into();
    let open = m.ext_libs[0]
        .externs
        .iter_mut()
        .find(|d| d.name == "open")
        .unwrap();
    let open_ts = open.langs.iter_mut().find(|l| l.lang == "ts").unwrap();
    open_ts.symbol = "new Dial<.summary>".into();
    let p = probe(&m, &m.ext_libs[0]);
    assert!(!p.source.contains("type TonoHandle_dial"), "{}", p.source);
    assert!(!p.source.contains(".summary"), "{}", p.source);
    for why in [
        "handle dial: #(Dial<.summary>) references .summary, a type the generated SDK defines",
        "method dial.read: #(Dial<.summary>) references .summary, a type the generated SDK defines",
        "op open: #(new Dial<.summary>) references .summary, a type the generated SDK defines",
    ] {
        assert!(p.skipped.contains(&why.to_string()), "{:?}", p.skipped);
    }
}

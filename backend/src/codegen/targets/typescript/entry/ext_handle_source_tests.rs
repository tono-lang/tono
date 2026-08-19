//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! its module-scoped seam reads the receiver off the in-progress draft,
//! projects `yields`/`returns` into the field's own type, and the field's
//! resolution step is the same one-line hand-off a free call takes. The
//! end-to-end `tsc` proof is `ts_ext_roundtrip::
//! a_field_sourced_from_a_handle_method_compiles_against_the_real_library`;
//! this module exercises the emitter's own branches directly.

use super::super::test_prelude::*;
use crate::codegen::fixtures::handle_source::handle_source_module;
use crate::ir::{Module, ShapeKind};

fn rendered_text(module: &Module) -> String {
    let emission = emit(module, &ts_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &TsRules)
}

#[test]
fn the_field_seam_reads_the_receiver_off_the_draft_and_projects_the_yields() {
    let out = rendered_text(&handle_source_module("ts"));
    assert!(
        out.contains("let configExt: (s: Settings) => Promise<Cfg> = async (s) => {"),
        "{out}"
    );
    assert!(out.contains("const raw = await s.provider.get();"), "{out}");
    assert!(
        out.contains("return { endpointRead: raw.readUrl, endpointWrite: raw.writeUrl };"),
        "{out}"
    );
    assert!(out.contains("s.config = await configExt(s);"), "{out}");
    // The argument reads its sibling off the same draft, and `@with` keeps
    // the injected value ahead of the call.
    assert!(
        out.contains("const raw = await s.provider.getFor(s.region);"),
        "{out}"
    );
    assert!(out.contains("s.scoped = await scopedExt(s);"), "{out}");
    // Construction became async because of the handle-method source alone
    // is not observable here (the free constructor call already forces
    // it); the op reading the same handle renders through its own seam,
    // off the resolved settings the method hands it.
    assert!(
        out.contains("return await probeHandleCall(this.settings);"),
        "{out}"
    );
    // Without a declared test there is no swapper to export.
    assert!(!out.contains("swapConfigExtForTest"), "{out}");
}

#[test]
fn a_tested_entry_exports_the_swapper_for_the_field_seam() {
    let mut module = handle_source_module("ts");
    module.tests.push(crate::ir::TestDecl {
        name: "constructs".into(),
        constructions: vec![crate::ir::TestConstruction {
            binding: "c".into(),
            entry: "client".into(),
            values: Default::default(),
        }],
        stubs: vec![],
        extern_stubs: vec![],
        calls: vec![],
        expects: vec![],
    });
    let out = rendered_text(&module);
    assert!(
        out.contains(
            "export function swapConfigExtForTest(next: typeof configExt): typeof configExt {"
        ),
        "{out}"
    );
}

#[test]
fn a_method_without_yields_narrows_the_raw_result_to_the_field_type() {
    let mut module = handle_source_module("ts");
    for method in &mut module.ext_libs[0].types[0].methods {
        method.langs[0].yields.clear();
        method.langs[0].returns = None;
    }
    let out = rendered_text(&module);
    assert!(out.contains("return raw as Cfg;"), "{out}");
}

#[test]
fn an_entry_whose_only_call_is_a_handle_method_source_still_constructs_asynchronously() {
    let mut module = handle_source_module("ts");
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            // The provider becomes injected (`@arg`), so the handle-method
            // source is the one thing left that awaits.
            let provider = fields.iter_mut().find(|f| f.name == "provider").unwrap();
            provider.call = None;
            provider.sources = vec![crate::ir::Source::Arg];
        }
    }
    let out = rendered_text(&module);
    assert!(out.contains("static async create("), "{out}");
    assert!(out.contains("s.config = await configExt(s);"), "{out}");
}

/// A handle-method stub on the method a `= .provider.get()` field reads:
/// the generated hermetic test swaps that field's own seam for the canned
/// logical value, alongside the constructor stub's fake handle (whose every
/// method, stubbed or not, only fails loudly: the stubbed one rides the seam).
#[test]
fn a_handle_method_stub_on_a_field_source_swaps_that_field_s_seam() {
    use crate::ir::{ExternStub, ExternStubTarget, StubAnswer, TestConstruction, TestDecl};

    let mut module = handle_source_module("ts");
    module.tests.push(TestDecl {
        name: "resolves the field from the stub".into(),
        constructions: vec![TestConstruction {
            binding: "c".into(),
            entry: "client".into(),
            values: Default::default(),
        }],
        stubs: vec![],
        extern_stubs: vec![
            ExternStub {
                binding: None,
                target: ExternStubTarget::Free {
                    lib: "envkit".into(),
                    fn_: "new_provider".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::json!({}),
                }],
            },
            ExternStub {
                binding: None,
                target: ExternStubTarget::Method {
                    lib: "envkit".into(),
                    ty: "provider".into(),
                    method: "get".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::json!({"endpoint_read": "r", "endpoint_write": "w"}),
                }],
            },
            // `scoped = .provider.get_for(.region)` is reached at construction
            // too; every reachable method must be covered for the test to plan.
            ExternStub {
                binding: None,
                target: ExternStubTarget::Method {
                    lib: "envkit".into(),
                    ty: "provider".into(),
                    method: "get_for".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::json!({"endpoint_read": "sr", "endpoint_write": "sw"}),
                }],
            },
        ],
        calls: vec![],
        expects: vec![],
    });
    crate::codegen::declared_tests::entry_tests(&module).expect("the test plans");
    let files = super::super::vector_tests::test_files(&module, &ts_casing());
    let hermetic = files
        .iter()
        .find(|f| f.group.tests_of() == Some(("client", false)))
        .expect("a hermetic test file");
    let out = rendered(&hermetic.file.decls, &TsRules);
    assert!(
        out.contains("swapConfigExtForTest(async () => decodeCfg("),
        "the field's seam must be swapped for the decoded logical answer: {out}"
    );
    assert!(
        out.contains("swapProviderExtForTest(async () => ({"),
        "{out}"
    );
    assert!(
        out.contains("envkit.provider.get: no stub for this call in test"),
        "{out}"
    );
    assert!(
        out.contains("envkit.provider.get_for: no stub for this call in test"),
        "{out}"
    );
    assert!(
        out.contains("swapScopedExtForTest(async () => decodeCfg("),
        "{out}"
    );
}

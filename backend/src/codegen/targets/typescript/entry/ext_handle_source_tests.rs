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
    // it); the op reading the same handle still renders off the client.
    assert!(
        out.contains("await this.settings.provider.getFor(this.settings.region)"),
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

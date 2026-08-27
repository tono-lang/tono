//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! its resolver takes the receiver (and every sibling an argument reads) as
//! a parameter, projects `yields`/`returns` into the field's own type, and
//! the field's resolution step is the same call-and-store a free call
//! takes, so the generated test runs the same resolver over its fake. The
//! end-to-end `tsc` proof is `ts_ext_roundtrip::
//! a_field_sourced_from_a_handle_method_compiles_against_the_real_library`;
//! this module exercises the emitter's own branches directly.

use super::super::test_prelude::*;
use crate::codegen::fixtures::handle_source::{handle_source_module, handle_source_test};
use crate::codegen::tree::{item_refs, Decl};
use crate::ir::{Module, ShapeKind, Tref};

fn rendered_text(module: &Module) -> String {
    let emission = emit(module, &ts_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    decls.extend(emission.ext.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &TsRules)
}

#[test]
fn the_field_resolver_takes_the_receiver_and_projects_the_yields() {
    let out = rendered_text(&handle_source_module("ts"));
    assert!(
        out.contains(
            "export async function resolveConfig(provider: ProviderHandle): Promise<Cfg> {"
        ),
        "{out}"
    );
    assert!(out.contains("const raw = await provider.get();"), "{out}");
    assert!(
        out.contains("return { endpointRead: raw.readUrl, endpointWrite: raw.writeUrl };"),
        "{out}"
    );
    assert!(
        out.contains(
            "const configValue = await resolveConfig(s.provider);\n    s.config = configValue;"
        ),
        "{out}"
    );
    // The argument is a parameter of its own, and `@with` keeps the
    // injected value ahead of the call.
    assert!(
        out.contains("export async function resolveScoped(provider: ProviderHandle, region: string): Promise<Cfg> {"),
        "{out}"
    );
    assert!(
        out.contains("const raw = await provider.getFor(region);"),
        "{out}"
    );
    assert!(
        out.contains("} else {\n      const scoped = await resolveScoped(s.provider, s.region);\n      s.scoped = scoped;"),
        "{out}"
    );
    // The op reading the same handle calls it inline, off the resolved
    // settings.
    assert!(
        out.contains("const raw = await this.settings.provider.getFor(this.settings.region);"),
        "{out}"
    );
    assert!(!out.contains("ForTest"), "{out}");
}

#[test]
fn a_tested_entry_exports_nothing_test_only() {
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
    assert!(!out.contains("ForTest"), "{out}");
    assert!(!out.contains("export let"), "{out}");
    // The steps a declared test composes are private statics of the class,
    // not exports: the barrel and the exports map never see them.
    assert!(out.contains("private static newSettings("), "{out}");
    assert!(
        out.contains("private static fromSettings(s: Settings): Client {"),
        "{out}"
    );
    // forTest is a different, intentionally public seam (a hand-written test
    // injects a transport over the real construction path): the entry has a
    // wire operation and a declared test, so it is still there.
    assert!(out.contains("static async forTest("), "{out}");
}

/// The `ts` binding of every provider method, with its `yields`/`returns`
/// projection dropped: the call answers the declared `cfg` itself.
fn without_projection() -> Module {
    let mut module = handle_source_module("ts");
    for method in &mut module.ext_libs[0].types[0].methods {
        method.langs[0].yields.clear();
        method.langs[0].returns = None;
    }
    module
}

/// A method with no `yields` answers the return the `.tono` declares, and
/// its interface says so: the generated `Cfg` shape, imported from the
/// module's own types (the interface lives in the entry's own file), never
/// `unknown`. The resolver then hands the raw result back with no cast.
#[test]
fn a_method_without_yields_declares_the_field_type_the_tono_declares() {
    let module = without_projection();
    let out = rendered_text(&module);
    assert!(out.contains("get(): Promise<Cfg>;"), "{out}");
    assert!(
        out.contains("getFor(region: string): Promise<Cfg>;"),
        "{out}"
    );
    assert!(!out.contains("unknown>"), "{out}");
    assert!(!out.contains("return raw as Cfg;"), "{out}");
    assert!(out.contains("return raw;"), "{out}");

    let emission = emit(&module, &ts_casing());
    let iface = emission
        .shared
        .iter()
        .chain(emission.per_entry.iter().flat_map(|(_, decls)| decls))
        .chain(emission.ext.iter().flat_map(|(_, decls)| decls))
        .find(
            |d| matches!(d, Decl::Raw(raw) if raw.text.contains("export interface ProviderHandle")),
        )
        .expect("the handle interface");
    assert!(
        item_refs(iface)
            .iter()
            .any(|s| s.name == "Cfg" && s.import.as_ref().map(|i| i.module.as_str()) == Some("kvs")),
        "the interface imports the shape it declares: {:?}",
        item_refs(iface)
    );
}

/// A method returning another handle of the same `ext` declares that
/// handle's own generated interface, the type its field would hold.
#[test]
fn a_method_returning_a_handle_declares_that_handle_s_interface() {
    let mut module = without_projection();
    let get_for = module.ext_libs[0].types[0]
        .methods
        .iter_mut()
        .find(|m| m.name == "get_for")
        .unwrap();
    get_for.r#return = Tref::Ref {
        id: "envkit#provider".into(),
        args: vec![],
    };
    let out = rendered_text(&module);
    assert!(
        out.contains("getFor(region: string): Promise<ProviderHandle>;"),
        "{out}"
    );
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
    assert!(
        out.contains("const configValue = await resolveConfig(s.provider);"),
        "{out}"
    );
}

/// A handle-method stub on the method a `= .provider.get()` field reads:
/// the generated hermetic test assigns the constructor stub's fake handle,
/// whose stubbed methods answer in the foreign shape (the projection
/// inverted), then runs the same resolver the factory runs over it.
#[test]
fn a_handle_method_stub_on_a_field_source_answers_through_the_fake_handle() {
    let mut module = handle_source_module("ts");
    module.tests.push(handle_source_test(
        "resolves the field from the stub",
        vec![],
    ));
    crate::codegen::declared_tests::entry_tests(&module).expect("the test plans");
    let files = super::super::vector_tests::test_files(&module, &ts_casing());
    let hermetic = files
        .iter()
        .find(|f| f.group.tests_of() == Some(("client", false)))
        .expect("a hermetic test file");
    let out = rendered(&hermetic.file.decls, &TsRules);
    assert!(out.contains("s.provider = {"), "{out}");
    assert!(
        out.contains("get: async () => ({ readUrl: \"r\", writeUrl: \"w\" }"),
        "the fake answers the foreign shape the field's projection reads: {out}"
    );
    assert!(
        out.contains("getFor: async () => ({ readUrl: \"sr\", writeUrl: \"sw\" }"),
        "{out}"
    );
    assert!(out.contains("} as ProviderHandle;"), "{out}");
    // The field resolves through the same resolver the factory runs, over
    // the fake.
    assert!(
        out.contains("const configValue = await resolveConfig(s.provider);"),
        "{out}"
    );
    assert!(out.contains("s.config = configValue;"), "{out}");
    assert!(
        out.contains("const scoped = await resolveScoped(s.provider, s.region);"),
        "{out}"
    );
    assert!(out.contains("s.scoped = scoped;"), "{out}");
    assert!(!out.contains("ForTest"), "{out}");
}

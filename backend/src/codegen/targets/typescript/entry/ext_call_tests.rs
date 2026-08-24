use super::super::test_prelude::*;
use super::*;
use crate::ir::{
    EntryField, ExtLib, ExternDecl, ForeignField, ForeignStruct, LangPath, OpaqueType, Prim,
    ReturnsField, ReturnsLit, Shape, ShapeKind, Source, Tref,
};

use super::super::ext_fixtures::ef;

/// A single-language (`ts`) module path, the shape every `ExtLib` in this
/// file declares.
fn ts_lang_path(path: &str) -> Vec<LangPath> {
    vec![LangPath {
        lang: "ts".into(),
        path: path.into(),
    }]
}

/// An `ext` library declaring only a `ts` binding: shared by every fixture
/// below so the `langs`/`name` boilerplate lives in one place.
fn ext_lib(
    name: &str,
    path: &str,
    structs: Vec<ForeignStruct>,
    types: Vec<OpaqueType>,
    externs: Vec<ExternDecl>,
) -> ExtLib {
    ExtLib {
        name: name.into(),
        langs: ts_lang_path(path),
        structs,
        types,
        externs,
    }
}

fn extern_param(name: &str, r#type: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type,
    }
}

fn string_param(name: &str) -> ExternParam {
    extern_param(name, Tref::Prim(Prim::String))
}

fn foreign_field(name: &str, r#type: Tref) -> ForeignField {
    ForeignField {
        name: name.into(),
        r#type,
    }
}

fn string_field(name: &str) -> ForeignField {
    foreign_field(name, Tref::Prim(Prim::String))
}

fn foreign_struct(name: &str, fields: Vec<ForeignField>) -> ForeignStruct {
    ForeignStruct {
        name: name.into(),
        langs: vec![],
        fields,
    }
}

fn app_config_shape() -> Shape {
    structure(
        "m#app_config",
        vec![
            member("endpoint", Tref::Prim(Prim::String), true),
            member("token", Tref::Prim(Prim::String), true),
        ],
    )
}

/// A worked `companyconfig`/`companybus` `ext` library pair: `load` (a
/// `Ctor` argument, `yields`+`returns` projecting foreign field names
/// onto `app_config`, a declared sentinel) and `connect` (a bare handle
/// construction, no `yields`).
fn appendix_ext_libs() -> Vec<ExtLib> {
    let companyconfig = ext_lib(
        "companyconfig",
        "@company/config",
        vec![
            foreign_struct(
                "ts_opts",
                vec![string_field("region"), string_field("service")],
            ),
            foreign_struct(
                "ts_config",
                vec![string_field("host"), string_field("token")],
            ),
        ],
        vec![],
        vec![super::super::ext_fixtures::load_config_extern(
            "m#app_config",
        )],
    );
    let companybus = ext_lib(
        "companybus",
        "@company/bus",
        vec![],
        vec![OpaqueType {
            name: "publisher".into(),
            langs: vec![crate::ir::ForeignLang {
                lang: "ts".into(),
                name: "Publisher".into(),
                fields: Default::default(),
            }],
            methods: vec![],
        }],
        vec![super::super::ext_fixtures::connect_publisher_extern(
            "companybus#publisher",
        )],
    );
    vec![companyconfig, companybus]
}

/// `service`/`region` (`@arg`), `config` (a plain call, `load`-shaped),
/// `bus` (a `@with`-fallback call onto the opaque handle,
/// `connect`-shaped, reading `config`'s own resolved members).
fn appendix_fields() -> Vec<EntryField> {
    let (config, bus) = super::super::ext_fixtures::appendix_config_and_bus_fields(
        "m#app_config",
        "companybus#publisher",
    );
    vec![
        ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None),
        ef("region", Tref::Prim(Prim::String), vec![Source::Arg], None),
        config,
        bus,
    ]
}

fn appendix_module(fields: Vec<EntryField>) -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![
            app_config_shape(),
            super::super::ext_fixtures::overloaded_shape("m"),
            Shape {
                id: "m#client".into(),
                kind: ShapeKind::Entry {
                    fields,
                    operations: vec![],
                },
                traits: vec![],
            },
        ],
        operations: vec![],
        extensions: vec![],
        ext_libs: appendix_ext_libs(),
    }
}

fn rendered_decls(module: &Module) -> Vec<Decl> {
    let emission = emit(module, &ts_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    decls
}

fn rendered_text(module: &Module) -> String {
    rendered(&rendered_decls(module), &TsRules)
}

#[test]
fn a_plain_call_field_awaits_the_foreign_symbol_through_its_seam() {
    // The import statement itself is a later file-assembly concern
    // (`repoint_to_groups`/`fill_symbol_slots`), not exercised by this
    // leaf-level harness; the ref that drives it is asserted separately
    // by checking the seam `Decl`'s own `refs`.
    let module = appendix_module(appendix_fields());
    let decls = rendered_decls(&module);
    let seam_decl = decls
        .iter()
        .find(|d| matches!(d, Decl::Raw(raw) if raw.text.contains("let configExt")))
        .expect("config seam decl");
    let refs = crate::codegen::tree::item_refs(seam_decl);
    assert!(
        refs.iter().any(|s| s.name == "load"
            && s.import
                .as_ref()
                .is_some_and(|i| i.module == "@company/config")),
        "the seam must import the foreign symbol `load` from its declared module: {refs:?}"
    );
    let out = rendered_text(&module);
    assert!(out.contains("const raw = await load("), "{out}");
    assert!(out.contains("s.config = await configExt(s);"), "{out}");
    // The `Ctor` argument's foreign field names ride verbatim, in the
    // ts language block's own order; the values are the resolved
    // sibling fields (`s.<field>`), not bare identifiers.
    assert!(
        out.contains("{ region: s.region, service: s.service }"),
        "{out}"
    );
}

#[test]
fn a_yields_projection_reads_the_foreign_verbatim_member_and_casts_to_the_logical_type() {
    let out = rendered_text(&appendix_module(appendix_fields()));
    assert!(
        out.contains("return { endpoint: raw.host, token: raw.token };"),
        "{out}"
    );
}

#[test]
fn a_bare_call_with_no_yields_assigns_the_raw_result_directly() {
    let out = rendered_text(&appendix_module(appendix_fields()));
    // The `@with` fallback wraps the call assignment inside its own
    // presence check; the leaf itself is still a bare pass-through
    // (through the seam).
    assert!(out.contains("s.bus = await busExt(s);"), "{out}");
    assert!(out.contains("await connect("), "{out}");
}

#[test]
fn a_declared_sentinel_throws_the_generated_typed_error() {
    let out = rendered_text(&appendix_module(appendix_fields()));
    assert!(
        out.contains("if (e instanceof BusyError) { throw new OverloadedError(e); }"),
        "{out}"
    );
    assert!(
        out.contains("export class OverloadedError extends TonoError"),
        "{out}"
    );
}

#[test]
fn an_unmapped_failure_falls_back_to_contract_error_naming_the_extern() {
    let out = rendered_text(&appendix_module(appendix_fields()));
    assert!(
        out.contains("throw new ContractError(\"companyconfig.load\", e);"),
        "{out}"
    );
    assert!(
        out.contains("throw new ContractError(\"companybus.connect\", e);"),
        "{out}"
    );
}

#[test]
fn an_entry_with_a_call_field_gets_an_async_static_factory_constructor() {
    let out = rendered_text(&appendix_module(appendix_fields()));
    let client_at = out.find("export class Client").expect("client class");
    let client_text = &out[client_at..];
    assert!(client_text.contains("private constructor("), "{out}");
    assert!(client_text.contains("static async create("), "{out}");
    assert!(!client_text.contains("\n  constructor("), "{out}");
}

#[test]
fn an_entry_with_no_call_field_keeps_the_plain_sync_constructor() {
    let fields = vec![ef(
        "service",
        Tref::Prim(Prim::String),
        vec![Source::Arg],
        None,
    )];
    let out = rendered_text(&appendix_module(fields));
    assert!(out.contains("\n  constructor("), "{out}");
    assert!(!out.contains("static async create("), "{out}");
    assert!(!out.contains("private constructor("), "{out}");
}

#[test]
fn no_foreign_form_is_exported_by_the_barrel() {
    let module = appendix_module(appendix_fields());
    let decls = rendered_decls(&module);
    let exports = crate::codegen::targets::typescript::emit::exports_of(&decls);
    for name in ["ts_opts", "ts_config", "load", "connect"] {
        assert!(
            !exports.values.iter().any(|v| v == name) && !exports.types.iter().any(|v| v == name),
            "the barrel must not export the foreign name {name:?}"
        );
    }
}

/// A declared test stubbing `companyconfig.load` (a free extern-fn call
/// reached during construction): the generated hermetic test swaps the
/// `config` field's seam for the canned logical answer, never awaiting
/// the real `load` import at all.
#[test]
fn a_free_extern_fn_stub_swaps_the_seam_in_the_generated_hermetic_test() {
    use crate::ir::{ExternStub, ExternStubTarget, StubAnswer, TestConstruction, TestDecl};
    use std::collections::BTreeMap;

    let mut module = appendix_module(appendix_fields());
    module.tests = vec![TestDecl {
        name: "loads config from the stub".into(),
        constructions: vec![TestConstruction {
            binding: "c".into(),
            entry: "client".into(),
            values: BTreeMap::from([
                ("service".to_string(), serde_json::json!("svc")),
                ("region".to_string(), serde_json::json!("us")),
            ]),
        }],
        stubs: vec![],
        extern_stubs: vec![
            ExternStub {
                binding: None,
                target: ExternStubTarget::Free {
                    lib: "companyconfig".into(),
                    fn_: "load".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::json!({"endpoint": "e", "token": "t"}),
                }],
            },
            // `bus` is a second free-fn extern call reachable during
            // construction (`companybus.connect`); every reachable
            // extern must be stubbed for the test to be planned at all.
            ExternStub {
                binding: None,
                target: ExternStubTarget::Free {
                    lib: "companybus".into(),
                    fn_: "connect".into(),
                },
                answers: vec![StubAnswer::Value {
                    value: serde_json::Value::Null,
                }],
            },
        ],
        calls: vec![],
        expects: vec![],
    }];
    let files = super::super::vector_tests::test_files(&module, &ts_casing());
    let hermetic = files
        .iter()
        .find(|f| f.group.tests_of() == Some(("client", false)))
        .expect("a hermetic test file");
    let out = rendered(&hermetic.file.decls, &TsRules);
    assert!(
        out.contains("swapConfigExtForTest"),
        "the test must swap the free-fn extern seam: {out}"
    );
    assert!(
        !out.contains("await load("),
        "a hermetic test must never reach the real import: {out}"
    );
}

/// A `match` inside `returns:` (the appendix's `.cfg.Env` example), the
/// same construct a config member's own `= match` selection uses,
/// lowered to an immediately invoked switch since TypeScript has no
/// match expression.
#[test]
fn a_match_inside_returns_lowers_to_an_immediately_invoked_switch() {
    use crate::ir::{ArmValue, Select, SelectArm};

    let mut fields = std::collections::BTreeMap::new();
    fields.insert("service".to_string(), CallArg::Param("service".into()));
    let lib = ext_lib(
        "companyconfig",
        "@company/config",
        vec![],
        vec![],
        vec![ExternDecl {
            name: "load".into(),
            params: vec![string_param("service")],
            r#return: Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            langs: vec![crate::ir::ExternLang {
                lang: "ts".into(),
                symbol: "load".into(),
                call_args: vec![CallArg::Param("service".into())],
                yields: vec![crate::ir::YieldsPos {
                    name: "cfg".into(),
                    r#type: None,
                    is_error: false,
                    foreign: None,
                }],
                returns: Some(ReturnsLit {
                    r#type: Tref::Ref {
                        id: "m#app_config".into(),
                        args: vec![],
                    },
                    fields: vec![ReturnsField {
                        name: "endpoint".into(),
                        value: ReturnsValue::Select(Select {
                            subject_index: None,
                            subject: vec!["cfg".into(), "Env".into()],
                            arms: vec![
                                SelectArm {
                                    pattern: Some(serde_json::json!("prod")),
                                    value: ArmValue::Field(vec!["cfg".into(), "Host".into()]),
                                },
                                SelectArm {
                                    pattern: None,
                                    value: ArmValue::Field(vec!["cfg".into(), "DevHost".into()]),
                                },
                            ],
                        }),
                    }],
                }),
                chain: None,
            }],
            r#async: vec!["ts".into()],
            errors: vec![],
        }],
    );
    let config = ef(
        "config",
        Tref::Ref {
            id: "m#app_config".into(),
            args: vec![],
        },
        vec![],
        Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![CallArg::Ref(vec!["service".into()])],
        }),
    );
    let service = ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None);
    let mut module = appendix_module(vec![service, config]);
    module.ext_libs = vec![lib];
    let out = rendered_text(&module);
    assert!(out.contains("return { endpoint: (() => {"), "{out}");
    assert!(out.contains("switch (raw.Env) {"), "{out}");
    assert!(out.contains("case \"prod\": return raw.Host;"), "{out}");
    assert!(out.contains("default: return raw.DevHost;"), "{out}");
    assert!(out.contains("})() };"), "{out}");
}

/// A static method (`#(Type.method)(args)`) is called on the imported type:
/// the seam imports the type the spelling names, not the method, and calls
/// `Type.method(..)` through it.
#[test]
fn a_static_method_receiver_imports_the_type_and_calls_its_member() {
    let mut module = appendix_module(appendix_fields());
    let load = &mut module.ext_libs[0].externs[0].langs[0];
    assert_eq!(load.symbol, "load");
    load.symbol = "ConfigLoader.load".into();
    let decls = rendered_decls(&module);
    let seam_decl = decls
        .iter()
        .find(|d| matches!(d, Decl::Raw(raw) if raw.text.contains("let configExt")))
        .expect("config seam decl");
    let refs = crate::codegen::tree::item_refs(seam_decl);
    assert!(
        refs.iter().any(|s| s.name == "ConfigLoader"
            && s.import
                .as_ref()
                .is_some_and(|i| i.module == "@company/config")),
        "the seam must import the receiver type: {refs:?}"
    );
    assert!(
        !refs.iter().any(|s| s.name == "load"),
        "the method itself is not an import: {refs:?}"
    );
    let out = rendered_text(&module);
    assert!(
        out.contains("const raw = await ConfigLoader.load("),
        "{out}"
    );
}

/// A class reference (`type handle`) passes the handle's class itself: the
/// seam imports the class from the lib's module and writes the identifier
/// where the argument goes, nested calls included.
#[test]
fn a_class_reference_imports_the_handle_class_and_passes_it() {
    let mut module = appendix_module(appendix_fields());
    let connect = &mut module.ext_libs[1].externs[0].langs[0];
    assert_eq!(connect.symbol, "connect");
    connect.call_args = vec![
        crate::ir::CallArg::TypeRef("publisher".into()),
        crate::ir::CallArg::SymbolCall(crate::ir::SymbolCall {
            symbol: "WithKind".into(),
            args: vec![crate::ir::CallArg::TypeRef("publisher".into())],
        }),
    ];
    let decls = rendered_decls(&module);
    let seam_decl = decls
        .iter()
        .find(|d| matches!(d, Decl::Raw(raw) if raw.text.contains("let busExt")))
        .expect("bus seam decl");
    let refs = crate::codegen::tree::item_refs(seam_decl);
    assert!(
        refs.iter().any(|s| s.name == "Publisher"
            && s.import
                .as_ref()
                .is_some_and(|i| i.module == "@company/bus")),
        "the seam must import the handle's class: {refs:?}"
    );
    let out = rendered_text(&module);
    assert!(
        out.contains("connect(Publisher, WithKind(Publisher))"),
        "{out}"
    );
}

/// The handle's `ts` block names the class the library exports, so a class
/// reference spells that name, verbatim, never the cased handle name.
#[test]
fn a_class_reference_spells_the_instantiation_ts_name_verbatim() {
    let mut module = appendix_module(appendix_fields());
    module.ext_libs[1].types[0].langs = vec![crate::ir::ForeignLang {
        lang: "ts".into(),
        name: "QueuePublisher".into(),
        fields: Default::default(),
    }];
    module.ext_libs[1].externs[0].langs[0].call_args =
        vec![crate::ir::CallArg::TypeRef("publisher".into())];
    let out = rendered_text(&module);
    assert!(out.contains("connect(QueuePublisher)"), "{out}");
    assert!(!out.contains("connect(Publisher)"), "{out}");
}

/// A parameter spelled under its own TypeScript type passes as the value
/// it is: the spelling is for the compiler to grade, structurally.
#[test]
fn a_spelled_parameter_renders_as_the_value_it_names() {
    let mut module = appendix_module(appendix_fields());
    let load = &mut module.ext_libs[0].externs[0].langs[0];
    load.call_args = load
        .call_args
        .iter()
        .map(|a| match a {
            CallArg::Param(name) => CallArg::ParamAs {
                name: name.clone(),
                spelling: "string".into(),
            },
            other => other.clone(),
        })
        .collect();
    let with_spelling = rendered_text(&module);
    let plain = rendered_text(&appendix_module(appendix_fields()));
    assert_eq!(with_spelling, plain);
}

#[test]
fn json_literal_renders_arrays_and_objects() {
    assert_eq!(
        json_literal(&serde_json::json!([1, "a", null])),
        "[1, \"a\", null]"
    );
    assert_eq!(json_literal(&serde_json::json!({"k": true})), "{ k: true }");
}

/// A word of a spelling is the library's whatever the module generates
/// under the same name: `Client` is imported from the library with and
/// without a `client` entry in the module. A generated type enters a
/// spelling only as a reference (`Memo<.reading>`), which is not imported
/// and renders as the generated name where the spelling is written.
#[test]
fn import_spelling_imports_a_word_that_collides_with_a_generated_type() {
    let mut module = appendix_module(appendix_fields());
    assert!(
        module
            .shapes
            .iter()
            .any(|s| crate::codegen::entries::local_name(&s.id) == "client"),
        "the fixture generates a Client of its own"
    );
    let lib = &module.ext_libs[0];
    let mut refs = Vec::new();
    import_spelling("Client", lib, &mut refs);
    assert!(
        refs.iter().any(|s| s.name == "Client"
            && s.import
                .as_ref()
                .is_some_and(|i| i.module == "@company/config")),
        "{refs:?}"
    );
    let mut refs = Vec::new();
    import_spelling("Memo<.reading>", lib, &mut refs);
    assert_eq!(
        refs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["Memo"]
    );
    module.shapes.push(Shape {
        id: "m#reading".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    });
    assert_eq!(spell("Memo<.reading>", &module), "Memo<Reading>");
    assert_eq!(spell("new Client", &module), "new Client");
}

/// A parameter spelled on the other side of the number/bigint divide
/// converts at the call: an i64 (a bigint here) crosses as `Number(..)`
/// when the binding says the library takes a number.
#[test]
fn a_spelled_parameter_converts_across_the_bigint_divide() {
    let mut module = appendix_module(appendix_fields());
    let connect = &mut module.ext_libs[1].externs[0];
    connect.params[0].r#type = Tref::Prim(Prim::I64);
    connect.langs[0].call_args[0] = CallArg::ParamAs {
        name: "endpoint".into(),
        spelling: "number".into(),
    };
    let out = rendered_text(&module);
    assert!(out.contains("connect(Number("), "{out}");
}

/// A form field the ts block spells under its own type converts inside the
/// struct literal, the same rule a spelled parameter follows.
#[test]
fn a_spelled_ctor_field_converts_inside_the_literal() {
    let mut module = appendix_module(appendix_fields());
    let opts = &mut module.ext_libs[0].structs[0];
    opts.fields[0].r#type = Tref::Prim(Prim::I64);
    opts.langs = vec![crate::ir::ForeignLang {
        lang: "ts".into(),
        name: "TsOpts".into(),
        fields: std::collections::BTreeMap::from([("region".to_string(), "number".to_string())]),
    }];
    let region = &mut module.ext_libs[0].externs[0];
    region.params[1].r#type = Tref::Prim(Prim::I64);
    let out = rendered_text(&module);
    assert!(out.contains("region: Number("), "{out}");
}

/// A struct literal under a spelling of its own passes as the literal it
/// is (an object literal is structural; the spelling is for `tsc` to grade
/// against the library), and a primitive spelling is refused by name before
/// generation, whether or not the form declares a ts block.
#[test]
fn a_spelled_ctor_literal_passes_structurally() {
    let mut module = appendix_module(appendix_fields());
    module.ext_libs[0].structs[0].langs = vec![crate::ir::ForeignLang {
        lang: "ts".into(),
        name: "TsOpts".into(),
        fields: Default::default(),
    }];
    let load = &mut module.ext_libs[0].externs[0];
    if let CallArg::Ctor(ctor) = &mut load.langs[0].call_args[0] {
        ctor.spelling = Some("Readonly<TsOpts>".into());
    } else {
        panic!("the appendix load call starts with its options literal");
    }
    let out = rendered_text(&module);
    assert!(out.contains("{ region:"), "{out}");
    assert!(!out.contains("Readonly"), "{out}");

    let form = &module.ext_libs[0].structs[0];
    assert!(
        crate::codegen::targets::typescript::entry::form_spelling_coerces(form, "Readonly<TsOpts>")
            .is_ok()
    );
    let err = crate::codegen::targets::typescript::entry::form_spelling_coerces(form, "number")
        .unwrap_err();
    assert!(err.contains("no conversion from TsOpts to number"), "{err}");
    let mut blockless = form.clone();
    blockless.langs.clear();
    let err =
        crate::codegen::targets::typescript::entry::form_spelling_coerces(&blockless, "number")
            .unwrap_err();
    assert!(
        err.contains(&format!("no conversion from {} to number", form.name)),
        "{err}"
    );
}

/// A `yields` list with no `returns:` names what the call returns and
/// projects nothing: the seam hands the raw result back as is, exactly as
/// a binding with no `yields` does.
#[test]
fn a_signature_yields_list_passes_the_raw_result_through() {
    let mut module = appendix_module(appendix_fields());
    let load = &mut module.ext_libs[0].externs[0].langs[0];
    assert!(!load.yields.is_empty());
    load.returns = None;
    let out = rendered_text(&module);
    assert!(!out.contains("raw.host"), "{out}");
    assert!(out.contains("return raw;"), "{out}");
}

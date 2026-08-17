use super::*;
use crate::codegen::targets::typescript::entry::emit;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::{member, rendered, structure};
use crate::ir::{
    EntryField, ExtLib, ExternDecl, ExternLang as IrExternLang, ForeignField, ForeignStruct,
    LangPath, OpaqueType, Prim, ReturnsField, ReturnsLit, Shape, ShapeKind, Source, Tref,
};
use std::collections::BTreeMap;

fn ef(name: &str, target: Tref, sources: Vec<Source>, call: Option<EntryCall>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

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
    let mut load_ctor_fields = BTreeMap::new();
    load_ctor_fields.insert("region".to_string(), CallArg::Param("region".into()));
    load_ctor_fields.insert("service".to_string(), CallArg::Param("service".into()));
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
        vec![ExternDecl {
            name: "load".into(),
            params: vec![string_param("service"), string_param("region")],
            r#return: Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            langs: vec![IrExternLang {
                lang: "ts".into(),
                symbol: "load".into(),
                call_args: vec![CallArg::Ctor(CallCtor {
                    name: "ts_opts".into(),
                    fields: load_ctor_fields,
                })],
                yields: vec![crate::ir::YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(Tref::Ref {
                        id: "companyconfig#ts_config".into(),
                        args: vec![],
                    }),
                    is_error: false,
                }],
                returns: Some(ReturnsLit {
                    r#type: Tref::Ref {
                        id: "m#app_config".into(),
                        args: vec![],
                    },
                    fields: vec![
                        ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Field(vec!["cfg".into(), "host".into()]),
                        },
                        ReturnsField {
                            name: "token".into(),
                            value: ReturnsValue::Field(vec!["cfg".into(), "token".into()]),
                        },
                    ],
                }),
                errors: vec![crate::ir::ErrorBinding {
                    sentinel: "BUSY".into(),
                    r#type: "overloaded".into(),
                }],
                sync: false,
                infallible: false,
                ctx: false,
            }],
        }],
    );
    let companybus = ext_lib(
        "companybus",
        "@company/bus",
        vec![],
        vec![OpaqueType {
            name: "publisher".into(),
            instance: None,
            methods: vec![],
        }],
        vec![ExternDecl {
            name: "connect".into(),
            params: vec![string_param("endpoint"), string_param("token")],
            r#return: Tref::Ref {
                id: "companybus#publisher".into(),
                args: vec![],
            },
            langs: vec![IrExternLang {
                lang: "ts".into(),
                symbol: "connect".into(),
                call_args: vec![
                    CallArg::Param("endpoint".into()),
                    CallArg::Param("token".into()),
                ],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: false,
            }],
        }],
    );
    vec![companyconfig, companybus]
}

/// `service`/`region` (`@arg`), `config` (a plain call, `load`-shaped),
/// `bus` (a `@with`-fallback call onto the opaque handle,
/// `connect`-shaped, reading `config`'s own resolved members).
fn appendix_fields() -> Vec<EntryField> {
    vec![
        ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None),
        ef("region", Tref::Prim(Prim::String), vec![Source::Arg], None),
        ef(
            "config",
            Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            vec![],
            Some(EntryCall {
                ns: "companyconfig".into(),
                func: "load".into(),
                args: vec![
                    CallArg::Ref(vec!["service".into()]),
                    CallArg::Ref(vec!["region".into()]),
                ],
            }),
        ),
        {
            let mut bus = ef(
                "bus",
                Tref::Ref {
                    id: "companybus#publisher".into(),
                    args: vec![],
                },
                vec![Source::With],
                Some(EntryCall {
                    ns: "companybus".into(),
                    func: "connect".into(),
                    args: vec![
                        CallArg::Ref(vec!["config".into(), "endpoint".into()]),
                        CallArg::Ref(vec!["config".into(), "token".into()]),
                    ],
                }),
            );
            bus.sources = vec![Source::With];
            bus
        },
    ]
}

fn appendix_module(fields: Vec<EntryField>) -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![
            app_config_shape(),
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
        out.contains("case \"BUSY\": throw new OverloadedError(e);"),
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
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: false,
            }],
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

//! Coverage for the `ext`/`extern` FFI emission (RFC-0023, [`super::ext`]):
//! the field-construction call, the injectable opaque handle, the op-level
//! method call, and the defensive fallbacks each lookup degrades to instead
//! of panicking on inconsistent IR. The end-to-end "does the Go actually
//! compile" proof lives in `backend/tests/go_ext_roundtrip.rs`; this module
//! exercises the emitter's own branches directly.

use super::*;
use crate::codegen::casing::CasingConfig;
use crate::codegen::entries::module_entries;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::rendered;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, ErrorBinding, ExtLib, ExternDecl, ExternLang,
    ExternParam, ForeignField, ForeignStruct, LangPath, Member, OpImplCall, OpaqueType, Prim,
    ReturnsField, ReturnsLit, ReturnsValue, Select, SelectArm, Shape, ShapeKind, Source, Trait,
    Tref, YieldsPos,
};

fn string_t() -> Tref {
    Tref::Prim(Prim::String)
}

fn member(name: &str, target: Tref) -> Member {
    Member {
        name: name.into(),
        target,
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

fn field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

fn ext_param(name: &str, target: Tref) -> ExternParam {
    ExternParam {
        name: name.into(),
        r#type: target,
    }
}

fn entry_text(module: &Module) -> String {
    let emission = emit(module, &go_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &GoRules::default())
}

// --- pure helpers -----------------------------------------------------

#[test]
fn lib_ident_keeps_a_legal_path_segment_verbatim() {
    assert_eq!(ext::lib_ident("github.com/company/config"), "config");
}

#[test]
fn lib_ident_replaces_illegal_characters_in_a_hyphenated_segment() {
    assert_eq!(
        ext::lib_ident("github.com/company/some-package"),
        "some_package"
    );
}

#[test]
fn lib_ident_prefixes_a_digit_leading_segment() {
    assert_eq!(ext::lib_ident("github.com/company/9lives"), "_9lives");
}

#[test]
fn lib_ident_falls_back_when_the_segment_is_empty() {
    assert_eq!(ext::lib_ident(""), "extlib");
}

#[test]
fn foreign_handle_detects_a_declared_opaque_type() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    let (lib, ty) = ext::foreign_handle(&t, &module).expect("declared handle");
    assert_eq!(lib.name, "bus");
    assert_eq!(ty, "publisher");
}

#[test]
fn foreign_handle_is_none_for_an_ordinary_shape_ref() {
    let module = bare_module();
    let t = Tref::Ref {
        id: "m#app_config".into(),
        args: vec![],
    };
    assert!(ext::foreign_handle(&t, &module).is_none());
}

#[test]
fn foreign_handle_is_none_for_a_non_ref_type() {
    let module = bare_module();
    assert!(ext::foreign_handle(&string_t(), &module).is_none());
}

#[test]
fn foreign_handle_is_none_when_the_lib_declares_no_such_type() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#subscriber".into(),
        args: vec![],
    };
    assert!(ext::foreign_handle(&t, &module).is_none());
}

#[test]
fn handle_go_type_spells_a_pointer_to_the_pascal_cased_name() {
    let lib = handle_lib("bus", "publisher");
    assert_eq!(
        ext::handle_go_type(&lib, "publisher"),
        Some("*bus.Publisher".to_string())
    );
}

#[test]
fn handle_go_type_is_none_without_a_go_module_path() {
    let mut lib = handle_lib("bus", "publisher");
    lib.langs.clear();
    assert!(ext::handle_go_type(&lib, "publisher").is_none());
    assert!(ext::handle_symbol(&lib).is_none());
}

#[test]
fn find_extern_reaches_a_method_on_an_opaque_handle() {
    let lib = handle_lib("bus", "publisher");
    let decl = ext::find_extern(&lib, "send").expect("method found");
    assert_eq!(decl.name, "send");
}

#[test]
fn find_lib_and_find_extern_miss_cleanly_when_unresolved() {
    let module = bare_module();
    assert!(ext::find_lib(&module, "nope").is_none());
    let lib = handle_lib("bus", "publisher");
    assert!(ext::find_extern(&lib, "nope").is_none());
}

fn bare_module() -> Module {
    Module {
        name: "m".into(),
        shapes: vec![structure(
            "m#app_config",
            vec![member("endpoint", string_t())],
        )],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
        tests: vec![],
    }
}

fn handle_lib(lib_name: &str, type_name: &str) -> ExtLib {
    ExtLib {
        name: lib_name.into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: format!("company/{lib_name}"),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: type_name.into(),
            methods: vec![ExternDecl {
                name: "send".into(),
                params: vec![ext_param("topic", string_t())],
                r#return: string_t(),
                langs: vec![ExternLang {
                    lang: "go".into(),
                    symbol: "Send".into(),
                    call_args: vec![CallArg::Param("topic".into())],
                    yields: vec![],
                    returns: None,
                    errors: vec![],
                }],
            }],
        }],
        externs: vec![],
    }
}

// --- call_arg_expr: every CallArg variant ------------------------------

#[test]
fn call_arg_expr_covers_every_variant() {
    let lib = handle_lib("bus", "publisher");
    let mut refs = Vec::new();
    let params = vec![ext_param("a", string_t())];
    let entry_args = vec![CallArg::Ref(vec!["region".into()])];
    let mut ref_expr = |path: &[String]| format!("s.{}", path.join("."));

    // Param resolves through entry_args.
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Param("a".into()),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "s.region"
    );
    // Param with no matching declared param (unreachable through `tono
    // check`, defended against here).
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Param("missing".into()),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "nil"
    );
    // Lit: string, bool, number, null.
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Lit(serde_json::json!("notes")),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "\"notes\""
    );
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Lit(serde_json::json!(true)),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "true"
    );
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Lit(serde_json::json!(3)),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "3"
    );
    assert_eq!(
        ext::call_arg_expr(
            &mut refs,
            &lib,
            &CallArg::Lit(serde_json::Value::Null),
            &params,
            &entry_args,
            &mut ref_expr
        ),
        "nil"
    );
    // List.
    let list = CallArg::List(vec![
        CallArg::Lit(serde_json::json!(1)),
        CallArg::Lit(serde_json::json!(2)),
    ]);
    assert_eq!(
        ext::call_arg_expr(&mut refs, &lib, &list, &params, &entry_args, &mut ref_expr),
        "[]any{1, 2}"
    );
    // Nested call: deferred.
    let nested = CallArg::Call(Box::new(EntryCall {
        ns: "other".into(),
        func: "sign".into(),
        args: vec![],
    }));
    assert!(ext::call_arg_expr(
        &mut refs,
        &lib,
        &nested,
        &params,
        &entry_args,
        &mut ref_expr
    )
    .contains("deferred"));
    // Ctor.
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("Topic".to_string(), CallArg::Lit(serde_json::json!("t")));
    let ctor = CallArg::Ctor(CallCtor {
        name: "publisher".into(),
        fields,
    });
    let expr = ext::call_arg_expr(&mut refs, &lib, &ctor, &params, &entry_args, &mut ref_expr);
    assert_eq!(expr, "bus.Publisher{Topic: \"t\"}");
    assert!(refs.iter().any(|s| s.name == "bus"));
}

#[test]
fn call_arg_expr_ctor_is_nil_without_a_go_module_path() {
    let mut lib = handle_lib("bus", "publisher");
    lib.langs.clear();
    let mut refs = Vec::new();
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("Topic".to_string(), CallArg::Lit(serde_json::json!("t")));
    let ctor = CallArg::Ctor(CallCtor {
        name: "publisher".into(),
        fields,
    });
    let mut ref_expr = |_: &[String]| String::new();
    assert_eq!(
        ext::call_arg_expr(&mut refs, &lib, &ctor, &[], &[], &mut ref_expr),
        "nil"
    );
}

// --- error_block --------------------------------------------------------

#[test]
fn error_block_with_no_sentinels_falls_back_to_contract_error() {
    let module = bare_module();
    let config = go_casing();
    let mut refs = Vec::new();
    let lib = handle_lib("bus", "publisher");
    let out = ext::error_block(
        &mut refs,
        &module,
        &config,
        &lib,
        &[],
        "bus.send",
        "err",
        &|expr| format!("return nil, {expr}"),
    );
    assert!(out.contains("if err != nil {"));
    assert!(out.contains("ContractName: \"bus.send\""));
    assert!(!out.contains("errors.Is"));
}

#[test]
fn error_block_discriminates_a_declared_sentinel() {
    let mut module = bare_module();
    module.shapes.push({
        let mut s = structure("m#overloaded", vec![member("message", string_t())]);
        s.traits = vec![Trait {
            id: "retryable".into(),
            value: serde_json::Value::Null,
        }];
        s
    });
    let config = go_casing();
    let mut refs = Vec::new();
    let lib = handle_lib("bus", "publisher");
    let out = ext::error_block(
        &mut refs,
        &module,
        &config,
        &lib,
        &[ErrorBinding {
            sentinel: "ErrBusy".into(),
            r#type: "overloaded".into(),
        }],
        "bus.send",
        "err",
        &|expr| format!("return nil, {expr}"),
    );
    assert!(out.contains("errors.Is(err, bus.ErrBusy)"));
    assert!(out.contains("&Overloaded{Message: err.Error()}"));
}

#[test]
fn declared_error_literal_is_a_zero_value_without_a_message_member() {
    let mut module = bare_module();
    module.shapes.push(structure("m#overloaded", vec![]));
    let config = go_casing();
    assert_eq!(
        ext::declared_error_literal(&module, &config, "overloaded", "err"),
        "&Overloaded{}"
    );
}

// --- call_assign / impl_call_body: happy paths and defensive fallbacks -

/// A single-entry module wiring a field-construction call, an injectable
/// handle with a construction fallback, and an op implemented by a call
/// into that handle's method with a declared sentinel: close to the RFC's
/// own appendix, exercised through the full `emit` pipeline so the
/// integration with `mod.rs`/`surface.rs`/`constructor.rs`/`resolve.rs` is
/// covered too, not just `ext.rs` in isolation.
fn appendix_like_module() -> Module {
    let app_config = structure(
        "m#app_config",
        vec![member("endpoint", string_t()), member("token", string_t())],
    );
    let ack = structure("m#ack", vec![member("id", string_t())]);
    let note = structure("m#note", vec![member("body", string_t())]);
    let mut overloaded = structure("m#overloaded", vec![member("message", string_t())]);
    overloaded.traits = vec![Trait {
        id: "retryable".into(),
        value: serde_json::Value::Null,
    }];

    let mut config_field = field(
        "config",
        Tref::Ref {
            id: "m#app_config".into(),
            args: vec![],
        },
        vec![],
    );
    config_field.call = Some(EntryCall {
        ns: "companyconfig".into(),
        func: "load".into(),
        args: vec![call_ref(&["region"])],
    });

    let mut bus_field = field(
        "bus",
        Tref::Ref {
            id: "companybus#publisher".into(),
            args: vec![],
        },
        vec![Source::With],
    );
    // Exercises Ctor, List, and Lit call-arg shapes together with a Ref, so
    // every `CallArg` branch a real field-construction call can carry is
    // covered through the full plan, not only the standalone unit test.
    let mut ctor_fields = std::collections::BTreeMap::new();
    ctor_fields.insert("Region".to_string(), call_ref(&["region"]));
    bus_field.call = Some(EntryCall {
        ns: "companybus".into(),
        func: "connect".into(),
        args: vec![
            CallArg::Ctor(CallCtor {
                name: "opts".into(),
                fields: ctor_fields,
            }),
            CallArg::List(vec![CallArg::Lit(serde_json::json!(1))]),
        ],
    });

    let region_field = field("region", string_t(), vec![Source::Arg]);

    let publish_op = Shape {
        id: "m#client.publish".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            input_name: Some("payload".into()),
            output: Some(Tref::Ref {
                id: "m#ack".into(),
                args: vec![],
            }),
            errors: vec![Tref::Ref {
                id: "m#overloaded".into(),
                args: vec![],
            }],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["bus".into()],
                method: "send".into(),
                args: vec![call_ref(&["payload", "body"])],
            }),
        },
        traits: vec![],
    };

    let entry = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![region_field, config_field, bus_field],
            operations: vec![publish_op],
        },
        traits: vec![],
    };

    let companyconfig = ExtLib {
        name: "companyconfig".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "company/config".into(),
        }],
        structs: vec![ForeignStruct {
            name: "go_config".into(),
            fields: vec![ForeignField {
                name: "Host".into(),
                r#type: string_t(),
            }],
        }],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: vec![ext_param("region", string_t())],
            r#return: Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Load".into(),
                call_args: vec![CallArg::Param("region".into())],
                yields: vec![YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(Tref::Ref {
                        id: "companyconfig#go_config".into(),
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
                            value: ReturnsValue::Field(vec!["cfg".into(), "Host".into()]),
                        },
                        ReturnsField {
                            name: "token".into(),
                            // A match projection, so the hoisted `switch`
                            // path renders too.
                            value: ReturnsValue::Select(Select {
                                subject: vec!["cfg".into(), "Host".into()],
                                arms: vec![
                                    SelectArm {
                                        pattern: Some(serde_json::json!("prod")),
                                        value: ArmValue::Lit(serde_json::json!("p")),
                                    },
                                    SelectArm {
                                        pattern: None,
                                        value: ArmValue::Field(vec!["cfg".into(), "Host".into()]),
                                    },
                                ],
                            }),
                        },
                    ],
                }),
                errors: vec![],
            }],
        }],
    };

    let companybus = ExtLib {
        name: "companybus".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "company/bus".into(),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: "publisher".into(),
            methods: vec![ExternDecl {
                name: "send".into(),
                params: vec![ext_param("body", string_t())],
                r#return: Tref::Ref {
                    id: "m#ack".into(),
                    args: vec![],
                },
                langs: vec![ExternLang {
                    lang: "go".into(),
                    symbol: "Send".into(),
                    call_args: vec![CallArg::Param("body".into())],
                    yields: vec![YieldsPos {
                        name: "a".into(),
                        r#type: Some(string_t()),
                        is_error: false,
                    }],
                    returns: Some(ReturnsLit {
                        r#type: Tref::Ref {
                            id: "m#ack".into(),
                            args: vec![],
                        },
                        fields: vec![ReturnsField {
                            name: "id".into(),
                            value: ReturnsValue::Field(vec!["a".into()]),
                        }],
                    }),
                    errors: vec![ErrorBinding {
                        sentinel: "ErrBusy".into(),
                        r#type: "overloaded".into(),
                    }],
                }],
            }],
        }],
        externs: vec![ExternDecl {
            name: "connect".into(),
            params: vec![
                ext_param("opts", string_t()),
                ext_param("extra", string_t()),
            ],
            r#return: Tref::Ref {
                id: "companybus#publisher".into(),
                args: vec![],
            },
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Connect".into(),
                call_args: vec![
                    CallArg::Param("opts".into()),
                    CallArg::Param("extra".into()),
                ],
                yields: vec![],
                returns: None,
                errors: vec![],
            }],
        }],
    };

    Module {
        name: "m".into(),
        shapes: vec![app_config, note, ack, overloaded, entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![companyconfig, companybus],
        tests: vec![],
    }
}

#[test]
fn the_appendix_like_module_wires_the_call_handle_and_impl_call() {
    let module = appendix_like_module();
    let text = entry_text(&module);

    // Field-construction call: import, call, yields naming, returns
    // projection with a hoisted match.
    assert!(text.contains(":= config.Load(s.Region)"));
    assert!(text.contains("configErr != nil"));
    assert!(text.contains("switch configCfg.Host {"));
    assert!(text.contains("var configToken string"));
    assert!(text.contains("s.Config = AppConfig{"));

    // The handle field is unexported, its With* option assigns without a
    // dereference (the carrier already holds the pointer type), and the
    // construction fallback wraps the call.
    assert!(text.contains("\tbus *bus.Publisher\n"));
    assert!(!text.contains("\tBus "));
    assert!(text.contains("func WithBus(v *bus.Publisher) ClientOption {"));
    assert!(text.contains("w.bus = v"));
    assert!(text.contains("if w.bus != nil {"));
    assert!(text.contains("s.bus = w.bus"));
    assert!(text.contains("bus.Opts{Region: s.Region}"));
    assert!(text.contains("[]any{1}"));
    assert!(text.contains(":= bus.Connect("));

    // The op's own impl call: reads the receiver, calls the method, maps
    // the declared sentinel, and projects the return.
    assert!(text.contains("c.settings.bus"));
    assert!(text.contains(".Send(input.Body)"));
    assert!(text.contains("errors.Is(publishErr, bus.ErrBusy)"));
    assert!(text.contains("&Overloaded{Message: publishErr.Error()}"));
    assert!(text.contains("ContractName: \"companybus.publisher.send\""));
    assert!(text.contains("return Ack{"));
}

// --- defensive fallbacks, exercised directly ---------------------------

#[allow(clippy::too_many_arguments)]
fn resolver_ctx<'a>(
    entry: &'a EntryModel<'a>,
    module: &'a Module,
    config: &'a CasingConfig,
    helpers: &'a mut Helpers,
    refs: &'a mut Vec<Symbol>,
    body: &'a mut String,
    resolve_fns: &'a mut Vec<Decl>,
) -> Resolver<'a, 'a> {
    Resolver {
        entry,
        module,
        config,
        helpers,
        refs,
        body,
        resolve_fns,
        multi: false,
    }
}

#[test]
fn call_assign_degrades_on_every_unresolved_lookup() {
    let mut module = bare_module();
    let mut config_field = field("config", string_t(), vec![]);
    config_field.call = Some(EntryCall {
        ns: "nope".into(),
        func: "load".into(),
        args: vec![],
    });
    let entry_shape = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![config_field.clone()],
            operations: vec![],
        },
        traits: vec![],
    };
    module.shapes.push(entry_shape);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = go_casing();
    let mut helpers = Helpers::default();
    let mut refs = Vec::new();
    let mut body = String::new();
    let mut resolve_fns = Vec::new();
    let mut r = resolver_ctx(
        entry,
        &module,
        &config,
        &mut helpers,
        &mut refs,
        &mut body,
        &mut resolve_fns,
    );
    let call = config_field.call.clone().unwrap();

    // Unresolved lib.
    let out = ext::call_assign(&mut r, &config_field, &call, "s.Config");
    assert!(out.contains("unresolved ext lib"));

    // Unresolved extern (lib exists, func doesn't).
    module.ext_libs.push(handle_lib("nope", "publisher"));
    let entries = module_entries(&module);
    let entry = &entries[0];
    let mut r = resolver_ctx(
        entry,
        &module,
        &config,
        &mut helpers,
        &mut refs,
        &mut body,
        &mut resolve_fns,
    );
    let out = ext::call_assign(&mut r, &config_field, &call, "s.Config");
    assert!(out.contains("unresolved extern"));

    // Extern exists but declares no Go binding.
    let ts_only = ExtLib {
        name: "nope".into(),
        langs: vec![LangPath {
            lang: "ts".into(),
            path: "@company/nope".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: vec![],
            r#return: string_t(),
            langs: vec![ExternLang {
                lang: "ts".into(),
                symbol: "load".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
            }],
        }],
    };
    module.ext_libs = vec![ts_only];
    let entries = module_entries(&module);
    let entry = &entries[0];
    let mut r = resolver_ctx(
        entry,
        &module,
        &config,
        &mut helpers,
        &mut refs,
        &mut body,
        &mut resolve_fns,
    );
    let out = ext::call_assign(&mut r, &config_field, &call, "s.Config");
    assert!(out.contains("declares no Go binding"));

    // Extern has a go binding, but the ext lib declares no Go module path.
    let no_go_path = ExtLib {
        name: "nope".into(),
        langs: vec![],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: vec![],
            r#return: string_t(),
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Load".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
            }],
        }],
    };
    module.ext_libs = vec![no_go_path];
    let entries = module_entries(&module);
    let entry = &entries[0];
    let mut r = resolver_ctx(
        entry,
        &module,
        &config,
        &mut helpers,
        &mut refs,
        &mut body,
        &mut resolve_fns,
    );
    let out = ext::call_assign(&mut r, &config_field, &call, "s.Config");
    assert!(out.contains("declares no Go module path"));
}

#[test]
fn call_assign_covers_an_explicit_error_position_and_a_bare_call_result() {
    let mut module = bare_module();
    let lib = ExtLib {
        name: "lib".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "company/lib".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "fetch".into(),
            params: vec![],
            r#return: string_t(),
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Fetch".into(),
                call_args: vec![],
                // The error position leads instead of trailing (ADR-0031
                // out-of-convention case), and there is no `returns:`,
                // so the bare call result assigns directly.
                yields: vec![
                    YieldsPos {
                        name: "err".into(),
                        r#type: None,
                        is_error: true,
                    },
                    YieldsPos {
                        name: "body".into(),
                        r#type: Some(string_t()),
                        is_error: false,
                    },
                ],
                returns: None,
                errors: vec![],
            }],
        }],
    };
    module.ext_libs = vec![lib];
    let mut config_field = field("config", string_t(), vec![]);
    config_field.call = Some(EntryCall {
        ns: "lib".into(),
        func: "fetch".into(),
        args: vec![],
    });
    let entry_shape = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![config_field.clone()],
            operations: vec![],
        },
        traits: vec![],
    };
    module.shapes.push(entry_shape);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = go_casing();
    let mut helpers = Helpers::default();
    let mut refs = Vec::new();
    let mut body = String::new();
    let mut resolve_fns = Vec::new();
    let mut r = resolver_ctx(
        entry,
        &module,
        &config,
        &mut helpers,
        &mut refs,
        &mut body,
        &mut resolve_fns,
    );
    let call = config_field.call.clone().unwrap();
    let out = ext::call_assign(&mut r, &config_field, &call, "s.Config");
    assert!(out.contains("configErr, configBody := lib.Fetch()"));
    assert!(out.contains("s.Config = configBody"));
}

#[test]
fn impl_call_body_degrades_on_every_unresolved_lookup() {
    let module = bare_module();
    // No entry at all in `bare_module`: build a throwaway one-field entry so
    // `EntryModel::fields` is non-empty for the "receiver not found" case,
    // and reuse it (unresolved) for "no receiver" too.
    let entry_shape = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![field("region", string_t(), vec![])],
            operations: vec![],
        },
        traits: vec![],
    };
    let mut with_entry = module.clone();
    with_entry.shapes.push(entry_shape);
    let entries = module_entries(&with_entry);
    let entry = &entries[0];
    let config = go_casing();
    let mut refs = Vec::new();

    // No receiver.
    let call = OpImplCall {
        recv: vec![],
        method: "send".into(),
        args: vec![],
    };
    let out = ext::impl_call_body(
        entry,
        &with_entry,
        &config,
        "publish",
        None,
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains("no receiver"));

    // Receiver field not found.
    let call = OpImplCall {
        recv: vec!["missing".into()],
        method: "send".into(),
        args: vec![],
    };
    let out = ext::impl_call_body(
        entry,
        &with_entry,
        &config,
        "publish",
        None,
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains("unresolved receiver field"));

    // Receiver field is not a foreign handle.
    let call = OpImplCall {
        recv: vec!["region".into()],
        method: "send".into(),
        args: vec![],
    };
    let out = ext::impl_call_body(
        entry,
        &with_entry,
        &config,
        "publish",
        None,
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains("is not a foreign handle field"));

    // A handle field whose method is not declared, and whose declared
    // method has no Go binding.
    let mut bus_module = with_entry.clone();
    bus_module.ext_libs = vec![handle_lib("bus", "publisher")];
    let mut bus_field = field(
        "bus",
        Tref::Ref {
            id: "bus#publisher".into(),
            args: vec![],
        },
        vec![],
    );
    bus_field.name = "bus".into();
    if let ShapeKind::Entry { fields, .. } = &mut bus_module.shapes.last_mut().unwrap().kind {
        fields.push(bus_field);
    }
    let entries = module_entries(&bus_module);
    let entry = &entries[0];
    let call = OpImplCall {
        recv: vec!["bus".into()],
        method: "missing".into(),
        args: vec![],
    };
    let out = ext::impl_call_body(
        entry,
        &bus_module,
        &config,
        "publish",
        None,
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains("unresolved method"));

    let mut ts_only_bus = bus_module.clone();
    ts_only_bus.ext_libs[0].types[0].methods[0].langs[0].lang = "ts".into();
    let entries = module_entries(&ts_only_bus);
    let entry = &entries[0];
    let call = OpImplCall {
        recv: vec!["bus".into()],
        method: "send".into(),
        args: vec![],
    };
    let out = ext::impl_call_body(
        entry,
        &ts_only_bus,
        &config,
        "publish",
        None,
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains("declares no Go binding"));
}

#[test]
fn impl_call_body_reads_a_ref_argument_off_the_input_param_and_off_an_entry_field() {
    let mut module = bare_module();
    let mut lib = handle_lib("bus", "publisher");
    lib.types[0].methods[0].params = vec![
        ext_param("body", string_t()),
        ext_param("region", string_t()),
    ];
    lib.types[0].methods[0].langs[0].call_args = vec![
        CallArg::Param("body".into()),
        CallArg::Param("region".into()),
    ];
    module.ext_libs = vec![lib];
    let region_field = field("region", string_t(), vec![]);
    let mut bus_field = field(
        "bus",
        Tref::Ref {
            id: "bus#publisher".into(),
            args: vec![],
        },
        vec![],
    );
    bus_field.name = "bus".into();
    let entry_shape = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![region_field, bus_field],
            operations: vec![],
        },
        traits: vec![],
    };
    module.shapes.push(entry_shape);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = go_casing();
    let mut refs = Vec::new();

    // Bare param reference (`.payload`, whole value) and a sibling entry
    // field reference (`.region`) in the same call.
    let call = OpImplCall {
        recv: vec!["bus".into()],
        method: "send".into(),
        args: vec![
            CallArg::Ref(vec!["payload".into()]),
            CallArg::Ref(vec!["region".into()]),
        ],
    };
    let out = ext::impl_call_body(
        entry,
        &module,
        &config,
        "publish",
        Some("payload"),
        &call,
        "nil, ",
        &|e| e,
        &mut refs,
    );
    assert!(out.contains(".Send(input, c.settings.Region)"));
}

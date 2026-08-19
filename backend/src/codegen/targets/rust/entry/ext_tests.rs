//! Coverage for the `ext`/`extern` FFI emission ([`super::ext`]): the
//! foreign-handle lookup, the handle's public/stored type spellings, and
//! the `impl .field.method(args)` call body. The end-to-end "does the Rust
//! actually compile" proof lives in `backend/tests/rust_ext_roundtrip.rs`;
//! this module exercises the emitter's own branches directly.

use super::super::ext_fixtures::rust_ext_fixture_model;
use super::*;
use crate::codegen::entries::module_entries;
use crate::codegen::ops::op_impl_call;
use crate::codegen::targets::rust::types::rust_casing;
use crate::ir::{
    CallCtor, EntryCall, EntryField, ErrorBinding, ExtLib, ExternDecl, ExternParam, Instance,
    LangPath, Member, Model, OpaqueType, Prim, ReturnsField, ReturnsLit, ReturnsValue, Shape,
    ShapeKind, Source, Tref, YieldsPos, TONO_IR_VERSION,
};

fn bare_module() -> Module {
    Module {
        name: "m".into(),
        shapes: vec![],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
        tests: vec![],
    }
}

fn handle_lib(lib: &str, ty: &str) -> ExtLib {
    ExtLib {
        name: lib.into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "some-bus".into(),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: ty.into(),
            interface: false,
            instance: None,
            methods: vec![],
        }],
        externs: vec![],
    }
}

#[test]
fn foreign_handle_detects_a_declared_opaque_type() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    let (lib, ty) = foreign_handle(&t, &module).expect("declared handle");
    assert_eq!(lib.name, "bus");
    assert_eq!(ty.name, "publisher");
}

#[test]
fn foreign_handle_is_none_for_an_ordinary_shape_ref() {
    let module = bare_module();
    let t = Tref::Ref {
        id: "m#app_config".into(),
        args: vec![],
    };
    assert!(foreign_handle(&t, &module).is_none());
}

#[test]
fn foreign_handle_is_none_for_a_non_ref_type() {
    let module = bare_module();
    assert!(foreign_handle(&Tref::Prim(Prim::String), &module).is_none());
}

#[test]
fn foreign_handle_is_none_when_the_lib_declares_no_such_type() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#subscriber".into(),
        args: vec![],
    };
    assert!(foreign_handle(&t, &module).is_none());
}

#[test]
fn foreign_handle_is_none_when_no_lib_matches_the_prefix() {
    let module = bare_module();
    let t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    assert!(foreign_handle(&t, &module).is_none());
}

#[test]
fn handle_rust_type_qualifies_the_pascal_cased_name_with_the_crate_path() {
    let lib = handle_lib("bus", "publisher");
    let ty = &lib.types[0];
    assert_eq!(
        handle_rust_type(&lib, ty),
        Some("some_bus::Publisher".to_string())
    );
}

#[test]
fn handle_rust_type_is_none_without_a_declared_rust_module_path() {
    let mut lib = handle_lib("bus", "publisher");
    lib.langs.clear();
    let ty = &lib.types[0];
    assert!(handle_rust_type(&lib, ty).is_none());
}

#[test]
fn field_type_spells_the_owned_handle_type_for_a_declared_handle() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    assert_eq!(field_type(&t, &module), "some_bus::Publisher");
}

#[test]
fn field_type_falls_back_to_the_ordinary_nominal_rendering_for_a_shape_ref() {
    let module = bare_module();
    let t = Tref::Prim(Prim::String);
    assert_eq!(field_type(&t, &module), rust_type(&t));
}

#[test]
fn settings_field_type_wraps_a_handle_in_option() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    assert_eq!(
        settings_field_type(&t, &module),
        "Option<some_bus::Publisher>"
    );
}

#[test]
fn settings_field_type_leaves_an_ordinary_type_unwrapped() {
    let module = bare_module();
    let t = Tref::Prim(Prim::String);
    assert_eq!(settings_field_type(&t, &module), rust_type(&t));
}

#[test]
fn is_stored_wrapped_is_true_only_for_a_declared_handle() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let handle_t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    assert!(is_stored_wrapped(&handle_t, &module));
    assert!(!is_stored_wrapped(&Tref::Prim(Prim::String), &module));
}

#[test]
fn wrap_stored_wraps_only_when_the_slot_is_option() {
    let mut module = bare_module();
    module.ext_libs = vec![handle_lib("bus", "publisher")];
    let handle_t = Tref::Ref {
        id: "bus#publisher".into(),
        args: vec![],
    };
    assert_eq!(wrap_stored(&handle_t, &module, "v"), "Some(v)");
    assert_eq!(wrap_stored(&Tref::Prim(Prim::String), &module, "v"), "v");
}

fn entry_text(model: &Model) -> String {
    use crate::codegen::pipeline::generate_target;
    use crate::codegen::{CodegenConfig, TargetKind};
    let files = generate_target(
        model,
        TargetKind::Rust,
        &CodegenConfig::default(),
        &rust_casing(),
    )
    .expect("the fixture model must generate cleanly");
    files
        .iter()
        .map(|f| f.text.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fixture's `publish` op has `impl .bus.send(..)` as its own body: the
/// generated method reads the stored handle back, diagnoses an unset one
/// instead of panicking, calls through it and projects the `yields`
/// binding into the declared `Ack` output.
#[test]
fn the_fixture_impl_call_op_reads_the_handle_and_projects_its_yields() {
    let model = rust_ext_fixture_model();
    let text = entry_text(&model);
    assert!(text.contains("recv.send("), "{text}");
    assert!(text.contains("ConfigError"), "{text}");
    assert!(text.contains("id: a.id"), "{text}");
    assert!(text.contains("accepted: a.accepted"), "{text}");
}

/// `impl_call_body` itself, exercised directly against the fixture's
/// `EntryModel` rather than through the full pipeline: the returned body is
/// relative to column zero and the `sync: false` extern declaration makes
/// the surrounding method `async`.
#[test]
fn impl_call_body_reports_async_for_a_non_sync_extern() {
    let model = rust_ext_fixture_model();
    let module = &model.modules[0];
    let entries = module_entries(module);
    let entry = entries
        .iter()
        .find(|e| e.name == "client")
        .expect("fixture declares a client entry");
    let op = entry
        .operations
        .iter()
        .find(|o| o.id.ends_with(".publish"))
        .expect("fixture declares a publish op");
    let call = op_impl_call(op).expect("publish op has an impl call");
    let config = rust_casing();
    let c = ImplCall {
        entry,
        module,
        config: &config,
        call,
        input_name: Some("payload"),
        has_output: true,
    };
    let (body, is_async) = impl_call_body(&c);
    assert!(is_async, "companybus.send is declared sync: false");
    assert!(body.contains("recv.send("), "{body}");
}

#[test]
fn a_render_of_the_full_fixture_is_stable_prose() {
    let model = rust_ext_fixture_model();
    let text = entry_text(&model);
    assert!(text.contains("companyconfig::load"), "{text}");
    // The `wrap_stored`/`Option` contract for a foreign handle slot: the
    // stored settings field type carries `Option<..>`.
    assert!(text.contains("Option<companybus::Publisher>"), "{text}");
}

#[test]
fn handle_rust_type_spells_a_generic_instance_with_its_foreign_name_and_type_argument() {
    let mut lib = handle_lib("bus", "typed_publisher");
    lib.types[0].instance = Some(Instance {
        foreign_name: "Publisher".into(),
        arg: Tref::Prim(Prim::String),
    });
    let ty = &lib.types[0];
    assert_eq!(
        handle_rust_type(&lib, ty),
        Some("some_bus::Publisher<String>".to_string())
    );
}

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

fn entry_field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// A minimal model whose `client` entry has an `input` (a struct with one
/// `body` member), a plain sibling `side` field, and a `conn` field typed by
/// a declared handle whose single method's own `langs[0].call_args` is
/// whatever the caller supplies -- the same knob Go's and TypeScript's own
/// `ArgCtx`-equivalent tests turn to exercise every `CallArg` variant.
fn handle_call_model(
    method_call_args: Vec<CallArg>,
    method_yields: Vec<YieldsPos>,
    method_returns: Option<ReturnsLit>,
    method_errors: Vec<ErrorBinding>,
    sync: bool,
) -> Model {
    let side = entry_field("side", Tref::Prim(Prim::String), vec![Source::Arg]);
    let conn = entry_field(
        "conn",
        Tref::Ref {
            id: "lib#h".into(),
            args: vec![],
        },
        vec![Source::With],
    );

    let op = Shape {
        id: "m#client.call_op".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#in".into(),
                args: vec![],
            }),
            input_name: Some("input".into()),
            output: Some(Tref::Ref {
                id: "m#out".into(),
                args: vec![],
            }),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: strings(&["conn"]),
                method: "do_it".into(),
                args: vec![CallArg::Ref(strings(&["input", "body"]))],
            }),
        },
        traits: vec![],
    };

    let client = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![side, conn],
            operations: vec![op],
        },
        traits: vec![],
    };

    let in_shape = Shape {
        id: "m#in".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![Member {
                name: "body".into(),
                target: Tref::Prim(Prim::String),
                required: true,
                default: None,
                constraints: vec![],
                traits: vec![],
            }],
        },
        traits: vec![],
    };
    let out_shape = Shape {
        id: "m#out".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![Member {
                name: "id".into(),
                target: Tref::Prim(Prim::String),
                required: true,
                default: None,
                constraints: vec![],
                traits: vec![],
            }],
        },
        traits: vec![],
    };

    let other_lib = ExtLib {
        name: "other".into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "other-crate".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "mk".into(),
            params: vec![],
            r#return: Tref::Prim(Prim::String),
            langs: vec![ExternLang {
                lang: "rust".into(),
                symbol: "mk".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: true,
                infallible: false,
                ctx: false,
            }],
        }],
    };

    let handle_lib = ExtLib {
        name: "lib".into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "some-handle".into(),
        }],
        structs: vec![],
        types: vec![OpaqueType {
            name: "h".into(),
            interface: false,
            instance: None,
            methods: vec![ExternDecl {
                name: "do_it".into(),
                params: vec![ExternParam {
                    name: "x".into(),
                    r#type: Tref::Prim(Prim::String),
                }],
                r#return: Tref::Ref {
                    id: "m#out".into(),
                    args: vec![],
                },
                langs: vec![ExternLang {
                    lang: "rust".into(),
                    symbol: "do_it".into(),
                    call_args: method_call_args,
                    yields: method_yields,
                    returns: method_returns,
                    errors: method_errors,
                    sync,
                    infallible: false,
                    ctx: false,
                }],
            }],
        }],
        externs: vec![],
    };

    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![Module {
            name: "m".into(),
            shapes: vec![client, in_shape, out_shape],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![other_lib, handle_lib],
            tests: vec![],
        }],
    }
}

fn call_op_body(model: &Model, has_output: bool) -> (String, bool) {
    let module = &model.modules[0];
    let entries = module_entries(module);
    let entry = entries
        .iter()
        .find(|e| e.name == "client")
        .expect("model declares a client entry");
    let op = entry
        .operations
        .iter()
        .find(|o| o.id.ends_with(".call_op"))
        .expect("model declares a call_op");
    let call = op_impl_call(op).expect("call_op has an impl call");
    let config = rust_casing();
    let c = ImplCall {
        entry,
        module,
        config: &config,
        call,
        input_name: Some("input"),
        has_output,
    };
    impl_call_body(&c)
}

/// Every non-`Ref`/`Param` `CallArg` variant `ArgCtx::arg_expr` handles,
/// plus a nested nullary extern call, all in one method's own declared
/// `call_args` -- the handle-call analogue of `resolve_call`'s own
/// "every call arg variant" coverage test.
#[test]
fn impl_call_body_renders_every_call_arg_variant() {
    let model = handle_call_model(
        vec![
            CallArg::Param("x".into()),
            CallArg::Ref(strings(&["side"])),
            CallArg::List(vec![CallArg::Lit(serde_json::json!(1))]),
            CallArg::Ctor(CallCtor {
                name: "Opts".into(),
                fields: [("n".to_string(), CallArg::Lit(serde_json::json!(3)))]
                    .into_iter()
                    .collect(),
            }),
            CallArg::Call(Box::new(EntryCall {
                ns: "other".into(),
                func: "mk".into(),
                args: vec![],
            })),
        ],
        vec![],
        None,
        vec![],
        false,
    );
    let (body, is_async) = call_op_body(&model, true);
    assert!(is_async);
    assert!(body.contains("(input.body).clone()"), "{body}");
    assert!(body.contains("(self.settings.side).clone()"), "{body}");
    assert!(body.contains("vec![1]"), "{body}");
    assert!(body.contains("Opts { n: 3 }"), "{body}");
    assert!(body.contains("other_crate::mk()"), "{body}");
    assert!(!body.contains("other_crate::mk().await"), "{body}");
}

/// A `Param` position with no positional `call.args` entry falls back to a
/// same-named field read instead of panicking.
#[test]
fn param_expr_falls_back_to_a_same_named_field_when_no_positional_arg_exists() {
    let mut model = handle_call_model(
        vec![CallArg::Param("side".into())],
        vec![],
        None,
        vec![],
        false,
    );
    if let ShapeKind::Entry { operations, .. } = &mut model.modules[0].shapes[0].kind {
        if let ShapeKind::Operation { impl_call, .. } = &mut operations[0].kind {
            impl_call.as_mut().unwrap().args = vec![];
        }
    }
    let (body, _) = call_op_body(&model, true);
    assert!(body.contains("(self.settings.side).clone()"), "{body}");
}

/// `sync: true` on the handle method's own binding drops the `.await` and
/// reports the surrounding method as sync.
#[test]
fn impl_call_body_reports_sync_for_a_sync_extern() {
    let model = handle_call_model(vec![], vec![], None, vec![], true);
    let (body, is_async) = call_op_body(&model, true);
    assert!(!is_async);
    assert!(!body.contains("recv.do_it().await"), "{body}");
    assert!(body.contains("recv.do_it()"), "{body}");
}

/// No `yields` at all binds the success value bare as `v` and hands it
/// straight back when the op has no declared output.
#[test]
fn impl_call_body_with_no_yields_and_no_output_returns_unit() {
    let model = handle_call_model(vec![], vec![], None, vec![], false);
    let (body, _) = call_op_body(&model, false);
    assert!(body.contains("Ok(v) => Ok(())"), "{body}");
}

/// More than one non-error `yields` position destructures a tuple out of
/// the single native success value.
#[test]
fn impl_call_body_with_multiple_yields_destructures_a_tuple() {
    let model = handle_call_model(
        vec![],
        vec![
            YieldsPos {
                name: "a".into(),
                r#type: Some(Tref::Prim(Prim::String)),
                is_error: false,
            },
            YieldsPos {
                name: "b".into(),
                r#type: Some(Tref::Prim(Prim::String)),
                is_error: false,
            },
        ],
        Some(ReturnsLit {
            r#type: Tref::Ref {
                id: "m#out".into(),
                args: vec![],
            },
            fields: vec![ReturnsField {
                name: "id".into(),
                value: ReturnsValue::Field(strings(&["a"])),
            }],
        }),
        vec![],
        false,
    );
    let (body, _) = call_op_body(&model, true);
    assert!(body.contains("Ok((a, b)) => Ok(Out { id: a })"), "{body}");
}

/// No declared `errors:` sentinel maps every failure straight to
/// `ContractError` (the `error_match` empty-arms fallback), never a
/// single-arm match.
#[test]
fn impl_call_body_with_no_declared_errors_falls_back_to_contract_error() {
    let model = handle_call_model(vec![], vec![], None, vec![], false);
    let (body, _) = call_op_body(&model, true);
    assert!(
        body.contains("Err(e) => Err(TonoError::Contract("),
        "{body}"
    );
    assert!(!body.contains("match e.to_string().as_str()"), "{body}");
}

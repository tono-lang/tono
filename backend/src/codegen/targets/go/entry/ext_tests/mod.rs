//! Coverage for the `ext`/`extern` FFI emission ([`super::ext`]):
//! the field-construction call, the injectable opaque handle, the op-level
//! method call, and the defensive fallbacks each lookup degrades to instead
//! of panicking on inconsistent IR. The end-to-end "does the Go actually
//! compile" proof lives in `backend/tests/go_ext_roundtrip.rs`; this module
//! exercises the emitter's own branches directly.

use super::ext_fixtures::{ext_param, field, member, string_t, structure};
use super::*;
use crate::codegen::entries::module_entries;
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::rendered;
use crate::ir::{
    CallArg, CallCtor, EntryCall, ErrorBinding, ExtLib, ExternDecl, ExternLang, LangPath,
    OpImplCall, OpaqueType, Shape, ShapeKind, Tref, YieldsPos,
};

fn entry_text(module: &Module) -> String {
    let emission = emit(module, &go_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &GoRules::default())
}

// --- pure helpers -----------------------------------------------------

#[test]
fn lib_ident_keeps_a_legal_lib_name_verbatim() {
    assert_eq!(ext::lib_ident("companyconfig"), "companyconfig");
}

#[test]
fn lib_ident_replaces_illegal_characters_in_a_hyphenated_name() {
    assert_eq!(ext::lib_ident("some-package"), "some_package");
}

#[test]
fn lib_ident_prefixes_a_digit_leading_name() {
    assert_eq!(ext::lib_ident("9lives"), "_9lives");
}

#[test]
fn lib_ident_falls_back_when_the_name_is_empty() {
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
            vec![member("endpoint", string_t(), true)],
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
                    sync: false,
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
        let mut s = structure("m#overloaded", vec![member("message", string_t(), true)]);
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

mod calls;

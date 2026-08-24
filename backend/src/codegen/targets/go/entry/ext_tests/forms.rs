//! A foreign struct literal in a `call:` argument: the form's own Go type
//! from its `go` block, its spelled fields converted, and the literal
//! itself converted when the argument spells how it crosses (`&Options`
//! for a library that takes the form by pointer). Split out of `ext_tests`
//! to keep it under the file-size ceiling; the fixtures (`bare_module`,
//! `handle_lib`) come from the parent module via `super::*`.

use super::*;

/// The bus library with one form, `opts`, whose `go` block names it
/// `Options` and spells `Digits` as `int`.
fn lib_with_options_form() -> ExtLib {
    let mut lib = handle_lib("bus", "publisher");
    let mut fields = std::collections::BTreeMap::new();
    fields.insert("Digits".to_string(), "int".to_string());
    lib.structs.push(crate::ir::ForeignStruct {
        name: "opts".into(),
        fields: vec![crate::ir::ForeignField {
            name: "Digits".into(),
            r#type: Tref::Prim(Prim::U8),
        }],
        langs: vec![ForeignLang {
            lang: "go".into(),
            name: "Options".into(),
            fields,
        }],
    });
    lib
}

fn options_literal(spelling: Option<&str>) -> CallArg {
    let mut ctor_fields = std::collections::BTreeMap::new();
    ctor_fields.insert("Digits".to_string(), CallArg::Lit(serde_json::json!(3)));
    CallArg::Ctor(CallCtor {
        name: "opts".into(),
        fields: ctor_fields,
        spelling: spelling.map(str::to_string),
    })
}

fn render(lib: &ExtLib, arg: &CallArg) -> String {
    let mut refs = Vec::new();
    let mut ref_expr = |_: &[String]| String::new();
    ext::call_arg_expr(
        &mut refs,
        &bare_module(),
        lib,
        arg,
        &[],
        &[],
        "ctx",
        &mut ref_expr,
    )
}

/// A struct literal names the form's own Go type from its `go` block, and
/// converts a field the block spells under another type.
#[test]
fn call_arg_expr_builds_a_form_from_its_go_block() {
    let lib = lib_with_options_form();
    assert_eq!(
        render(&lib, &options_literal(None)),
        "bus.Options{Digits: int(3)}"
    );
}

/// The literal under a spelling of its own: `&Options` takes the address
/// of the literal the library's constructor wants a pointer to, and the
/// form's own type passes it unchanged. The `&` is the argument's, never
/// the form's: the block still names `Options`.
#[test]
fn a_spelled_form_literal_crosses_as_the_spelling_asks() {
    let lib = lib_with_options_form();
    assert_eq!(
        render(&lib, &options_literal(Some("&Options"))),
        "&bus.Options{Digits: int(3)}"
    );
    assert_eq!(
        render(&lib, &options_literal(Some("Options"))),
        "bus.Options{Digits: int(3)}"
    );
    assert_eq!(lib.structs[0].lang("go").unwrap().name, "Options");
}

/// A spelling Go cannot reach from the literal is refused naming both
/// types, before any emitter could reach the panic in `ctor_expr`; a form
/// with no `go` block is refused on its own, not here.
#[test]
fn a_form_spelling_with_no_conversion_is_refused_naming_both_types() {
    let lib = lib_with_options_form();
    let module = bare_module();
    let coerces = |spelling: &str| {
        crate::codegen::targets::go::entry::form_spelling_coerces(
            &module,
            &lib,
            &lib.structs[0],
            spelling,
        )
    };
    assert!(coerces("&Options").is_ok());
    assert!(coerces("Options").is_ok());
    let err = coerces("*Options").unwrap_err();
    assert!(
        err.contains("no conversion from bus.Options to *Options"),
        "{err}"
    );
    let err = coerces("&Settings").unwrap_err();
    assert!(
        err.contains("cannot pass a bus.Options literal as &Settings"),
        "{err}"
    );
    let mut blockless = lib.structs[0].clone();
    blockless.langs.clear();
    assert!(crate::codegen::targets::go::entry::form_spelling_coerces(
        &module, &lib, &blockless, "&Options"
    )
    .is_ok());
}

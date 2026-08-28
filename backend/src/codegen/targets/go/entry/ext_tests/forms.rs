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

/// The module with one wire struct, `reading`, whose `sample_rate` member
/// is renamed for Go and whose `note` member is optional.
fn module_with_reading() -> Module {
    let mut module = bare_module();
    module.shapes.push(structure(
        "m#reading",
        vec![
            member("value", Tref::Prim(Prim::Float), true),
            crate::codegen::test_support::member_with(
                "sample_rate",
                Tref::Prim(Prim::U32),
                true,
                vec![crate::codegen::test_support::trait_of(
                    "core#rename",
                    serde_json::json!({ "go": "Hz" }),
                )],
            ),
            member("note", string_t(), false),
        ],
    ));
    module
}

fn reading_literal(fields: &[(&str, &[&str])]) -> CallArg {
    CallArg::Ctor(CallCtor {
        name: "reading".into(),
        fields: fields
            .iter()
            .map(|(k, path)| {
                (
                    (*k).to_string(),
                    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect()),
                )
            })
            .collect(),
        spelling: None,
    })
}

/// A literal naming none of the lib's forms builds one of the module's own
/// structs: the generated type's composite literal, its fields named as
/// the types file names them (cased, `@rename(go)` honored), no package
/// selector, and an optional member left out keeps Go's zero value.
#[test]
fn a_literal_of_a_generated_struct_is_the_generated_types_own_literal() {
    let lib = lib_with_options_form();
    let module = module_with_reading();
    let mut refs = Vec::new();
    let mut ref_expr = |path: &[String]| format!("s.{}", path.join("."));
    let out = ext::call_arg_expr(
        &mut refs,
        &module,
        &lib,
        &reading_literal(&[("value", &["v"]), ("sample_rate", &["rate"])]),
        &[],
        &[],
        "ctx",
        &mut ref_expr,
    );
    assert_eq!(out, "Reading{Hz: s.rate, Value: s.v}");
}

/// The literal written at the call site (`remember(reading { .. })`)
/// reaches the block's `call:` line through the `Param` it substitutes,
/// the same substitution a reference to a field of that type goes
/// through: the two render into the same position, differing only in the
/// argument expression.
#[test]
fn a_generated_literal_and_a_reference_substitute_the_same_param() {
    let lib = lib_with_options_form();
    let module = module_with_reading();
    let params = vec![ext_param(
        "seed",
        Tref::Ref {
            id: "m#reading".into(),
            args: vec![],
        },
    )];
    let render_site = |site_arg: CallArg| {
        let mut refs = Vec::new();
        let mut ref_expr = |path: &[String]| format!("s.{}", path.join("."));
        ext::call_arg_expr(
            &mut refs,
            &module,
            &lib,
            &CallArg::Param("seed".into()),
            &params,
            &[site_arg],
            "ctx",
            &mut ref_expr,
        )
    };
    assert_eq!(render_site(CallArg::Ref(vec!["seed".into()])), "s.seed");
    assert_eq!(
        render_site(reading_literal(&[
            ("value", &["v"]),
            ("sample_rate", &["rate"])
        ])),
        "Reading{Hz: s.rate, Value: s.v}"
    );
}

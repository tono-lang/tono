//! A struct literal at an extern call site naming one of the module's own
//! wire structs (`combined = lib.make(reading { .. })`): built when the
//! struct is a non-generic wire struct with its required members present,
//! refused by name otherwise. Split out of `validate_gates` to keep it
//! under the file-size ceiling; the fixtures come from that module and the
//! parent via `super::*`.

use super::validate_gates::{constructed_primary, model_with_forwarding_call};
use super::*;
use crate::codegen::output::TargetKind;
use crate::ir::{CallArg, CallCtor, ExternLang};

/// `combined = lib.make(reading { .. })` over the shared fixture, with the
/// module also declaring the wire struct `reading` (`value` required,
/// `note` optional) and `make` bound for every target: the literal is the
/// caller's own argument, so it is only ever seen through the site.
fn model_with_generated_literal(ctor: CallCtor) -> crate::ir::Model {
    use crate::codegen::test_support::{member, structure};
    let mut m = model_with_forwarding_call(constructed_primary(), vec![CallArg::Ctor(ctor)]);
    m.modules[0].shapes.push(structure(
        "m#reading",
        vec![
            member("value", Tref::Prim(crate::ir::Prim::Float), true),
            member("note", Tref::Prim(crate::ir::Prim::String), false),
        ],
    ));
    let make = &mut m.modules[0].ext_libs[0].externs[0];
    for lang in ["ts", "rust"] {
        make.langs.push(ExternLang {
            lang: lang.into(),
            symbol: "make".into(),
            call_args: vec![],
            yields: vec![],
            returns: None,
            chain: None,
        });
    }
    m
}

fn reading_literal(fields: &[&str], spelling: Option<&str>) -> CallCtor {
    CallCtor {
        name: "reading".into(),
        fields: fields
            .iter()
            .map(|f| ((*f).to_string(), CallArg::Lit(serde_json::json!(1.5))))
            .collect(),
        spelling: spelling.map(str::to_string),
    }
}

const ALL_TARGETS: [TargetKind; 3] = [TargetKind::Go, TargetKind::Rust, TargetKind::TypeScript];

/// A literal at the call site naming one of the module's own wire structs
/// validates for every target once its required members are present; an
/// optional member left out is fine (absent in the generated literal).
#[test]
fn a_generated_struct_literal_at_the_call_site_validates_for_every_target() {
    let m = model_with_generated_literal(reading_literal(&["value"], None));
    assert!(validate_entries(&m, &ALL_TARGETS).is_ok());
    let m = model_with_generated_literal(reading_literal(&["value", "note"], None));
    assert!(validate_entries(&m, &ALL_TARGETS).is_ok());
}

/// The literal is refused, naming the site and the reason, when it cannot
/// be built: a field the struct does not declare, a required member left
/// out, a spelling on it, or a name that is no struct at all. None of
/// these reaches an emitter (which would have looked for a foreign block
/// the struct never declares).
#[test]
fn a_generated_struct_literal_that_cannot_be_built_is_refused_by_name() {
    let refuse = |ctor: CallCtor| {
        let m = model_with_generated_literal(ctor);
        let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
        assert!(err.contains("combined = lib.make(..)"), "{err}");
        err
    };
    let err = refuse(reading_literal(&["value", "unit"], None));
    assert!(
        err.contains("struct reading declares no field unit"),
        "{err}"
    );
    let err = refuse(reading_literal(&["note"], None));
    assert!(
        err.contains("leaves out value, which struct reading requires"),
        "{err}"
    );
    let err = refuse(reading_literal(&["value"], Some("&.reading")));
    assert!(err.contains("cannot be spelled"), "{err}");
    let mut unknown = reading_literal(&[], None);
    unknown.name = "nothing".into();
    let err = refuse(unknown);
    assert!(
        err.contains("names neither a struct of ext lib nor a struct of module m"),
        "{err}"
    );
    let mut entry = reading_literal(&[], None);
    entry.name = "client".into();
    let err = refuse(entry);
    assert!(err.contains("client is not a wire struct"), "{err}");
}

/// A generic struct has no literal without its type arguments, so a literal
/// naming one is refused before any target could write an instantiation it
/// never declared.
#[test]
fn a_generic_struct_literal_is_refused() {
    let mut m = model_with_generated_literal(reading_literal(&["value"], None));
    if let ShapeKind::Structure { params, .. } = &mut m.modules[0].shapes[1].kind {
        params.push("T".into());
    } else {
        panic!("the reading struct is the second shape");
    }
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("reading is a generic struct"), "{err}");
}

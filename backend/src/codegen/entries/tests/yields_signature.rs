//! The two readings of a `yields` list at generation time, reasserted over
//! hand-fed IR: with no `returns:` the list is the call's signature and its
//! value position must be the op's own return; a `returns:` never builds an
//! opaque handle. Split out of `validate_gates` to keep it under the
//! file-size ceiling; the handle fixtures come from there.

use super::validate_gates::{call_ref, constructed_primary, model_with_forwarding_call};
use crate::codegen::entries::validate_entries;
use crate::codegen::output::TargetKind;
use crate::ir::Tref;

/// A `yields` list with no `returns:` is the call's signature: its value
/// position must be the op's own return (then nothing is dead), any other
/// type has nothing to project it into, and a `returns:` can never build
/// an opaque handle.
#[test]
fn a_signature_yields_list_is_consumed_by_the_op_s_own_return() {
    let position = |t: Tref| crate::ir::YieldsPos {
        name: "c".into(),
        r#type: Some(t),
        is_error: false,
        foreign: None,
    };
    let handle = Tref::Ref {
        id: "lib#handle".into(),
        args: vec![],
    };

    let mut m = model_with_forwarding_call(constructed_primary(), vec![call_ref(&["primary"])]);
    m.modules[0].ext_libs[0].externs[0].langs[0].yields = vec![position(handle.clone())];
    assert!(validate_entries(&m, &[TargetKind::Go]).is_ok());

    let mut m = model_with_forwarding_call(constructed_primary(), vec![call_ref(&["primary"])]);
    m.modules[0].ext_libs[0].externs[0].langs[0].yields =
        vec![position(Tref::Prim(crate::ir::Prim::String))];
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(
        err.contains("yields position c") && err.contains("not the op's own return"),
        "{err}"
    );

    let mut m = model_with_forwarding_call(constructed_primary(), vec![call_ref(&["primary"])]);
    let lang = &mut m.modules[0].ext_libs[0].externs[0].langs[0];
    lang.yields = vec![position(handle.clone())];
    lang.returns = Some(crate::ir::ReturnsLit {
        r#type: handle,
        fields: vec![crate::ir::ReturnsField {
            name: "c".into(),
            value: crate::ir::ReturnsValue::Field(vec!["c".into()]),
        }],
    });
    let err = validate_entries(&m, &[TargetKind::Go]).unwrap_err();
    assert!(err.contains("builds handle, an opaque handle"), "{err}");
}

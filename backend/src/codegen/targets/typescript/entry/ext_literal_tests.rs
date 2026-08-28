//! A struct literal in a `ts` block's `call:` line naming one of the
//! module's own structs, rendered as an object literal under the generated
//! interface's own keys. Split out of `ext_call_tests` to keep it under the
//! file-size ceiling; the harness (`appendix_module`, `rendered_text`) comes
//! from there.

use super::tests::{appendix_fields, appendix_module, rendered_text};
use crate::codegen::test_support::{member, member_with, structure, trait_of};
use crate::ir::{CallArg, CallCtor, Prim, Tref};

/// A literal naming none of the lib's forms builds one of the module's own
/// structs: an object literal is structural, so no name is written, but
/// its keys are the interface's own (cased by the types file, `@rename(ts)`
/// honored), never the canonical names the ctor carries.
#[test]
fn a_literal_of_a_generated_struct_uses_the_interfaces_own_keys() {
    let mut module = appendix_module(appendix_fields());
    module.shapes.push(structure(
        "m#reading",
        vec![
            member("region_code", Tref::Prim(Prim::String), true),
            member_with(
                "service",
                Tref::Prim(Prim::String),
                true,
                vec![trait_of("core#rename", serde_json::json!({ "ts": "svc" }))],
            ),
        ],
    ));
    let load = &mut module.ext_libs[0].externs[0];
    load.langs[0].call_args[0] = CallArg::Ctor(CallCtor {
        name: "reading".into(),
        fields: std::collections::BTreeMap::from([
            ("region_code".to_string(), CallArg::Param("region".into())),
            ("service".to_string(), CallArg::Param("service".into())),
        ]),
        spelling: None,
    });
    let out = rendered_text(&module);
    assert!(
        out.contains("load({ regionCode: region, svc: service })"),
        "{out}"
    );
}

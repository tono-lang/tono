//! The extern-call field-source tests (RFC-0023): DAG ordering across a call
//! chain, cycle detection over call-arg refs, `is_guaranteed` classification
//! for plain and `@with`-fallback call fields, and `FieldShape::Call`
//! dispatch regardless of the target's own shape. Fixtures (`field`,
//! `entry_shape`, `module_of`) come from the parent module via `super::*`.

use super::*;
use crate::ir::{CallArg, EntryCall, ExtLib, OpaqueType};

fn call_field(name: &str, ns: &str, func: &str, args: Vec<CallArg>) -> EntryField {
    let mut f = field(name, vec![]);
    f.call = Some(EntryCall {
        ns: ns.into(),
        func: func.into(),
        args,
    });
    f
}

fn call_ref(path: &[&str]) -> CallArg {
    CallArg::Ref(path.iter().map(|s| (*s).to_string()).collect())
}

#[test]
fn a_three_level_call_chain_orders_with_no_declared_order() {
    // config reads .service/.region; auth and bus each read .config. Declared
    // out of dependency order, on purpose, so the DAG (not declaration order)
    // has to produce the chain.
    let auth = call_field("auth", "companyauth", "sign", vec![call_ref(&["config"])]);
    let bus = call_field(
        "bus",
        "companybus",
        "connect",
        vec![
            call_ref(&["config", "endpoint"]),
            call_ref(&["config", "token"]),
        ],
    );
    let config = call_field(
        "config",
        "companyconfig",
        "load",
        vec![call_ref(&["service"]), call_ref(&["region"])],
    );
    let service = field("service", vec![Source::Default(json!("notes"))]);
    let region = field("region", vec![Source::Arg]);
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![auth, bus, config, service, region],
    )]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    let pos = |n: &str| order.iter().position(|x| *x == n).unwrap();
    assert!(pos("service") < pos("config"));
    assert!(pos("region") < pos("config"));
    assert!(pos("config") < pos("auth"));
    assert!(pos("config") < pos("bus"));
}

#[test]
fn a_cycle_between_call_sourced_fields_appends_in_declaration_order_instead_of_dropping() {
    let a = call_field("a", "ns", "f", vec![call_ref(&["b"])]);
    let b = call_field("b", "ns", "g", vec![call_ref(&["a"])]);
    let module = module_of(vec![entry_shape("m#client", vec![a, b])]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["a", "b"]);
}

#[test]
fn a_plain_call_field_is_guaranteed_when_its_args_are() {
    let region = field("region", vec![Source::Arg]);
    let config = call_field("config", "ns", "load", vec![call_ref(&["region"])]);
    let module = module_of(vec![entry_shape("m#client", vec![config, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = entry.fields.iter().find(|f| f.name == "config").unwrap();
    assert!(entry.is_guaranteed(config));
}

#[test]
fn a_plain_call_field_is_not_guaranteed_when_it_reads_a_non_guaranteed_sibling() {
    let region = field("region", vec![Source::Env(EnvName::Name("REGION".into()))]);
    let config = call_field("config", "ns", "load", vec![call_ref(&["region"])]);
    let module = module_of(vec![entry_shape("m#client", vec![config, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let config = entry.fields.iter().find(|f| f.name == "config").unwrap();
    assert!(!entry.is_guaranteed(config));
}

#[test]
fn a_with_field_backed_by_a_guaranteed_call_fallback_is_guaranteed() {
    // decision G: an injectable handle with construction as fallback is
    // guaranteed the same way a plain call is (either the caller injects it,
    // or the fallback call runs and its own reads are guaranteed) — `@with`
    // does not relax the rule, both reduce to the same check.
    let region = field("region", vec![Source::Arg]);
    let mut bus = call_field("bus", "companybus", "connect", vec![call_ref(&["region"])]);
    bus.sources = vec![Source::With];
    let module = module_of(vec![entry_shape("m#client", vec![bus, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let bus = entry.fields.iter().find(|f| f.name == "bus").unwrap();
    assert!(entry.is_guaranteed(bus));
}

#[test]
fn a_with_field_backed_by_a_non_guaranteed_call_fallback_is_not_guaranteed() {
    let region = field("region", vec![Source::Env(EnvName::Name("REGION".into()))]);
    let mut bus = call_field("bus", "companybus", "connect", vec![call_ref(&["region"])]);
    bus.sources = vec![Source::With];
    let module = module_of(vec![entry_shape("m#client", vec![bus, region])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let bus = entry.fields.iter().find(|f| f.name == "bus").unwrap();
    assert!(!entry.is_guaranteed(bus));
}

#[test]
fn a_call_sourced_field_reports_the_call_shape_regardless_of_its_target() {
    let plain = call_field("config", "ns", "load", vec![]);
    let mut opaque = call_field("bus", "ns", "connect", vec![]);
    // An opaque handle's type has no entry in `module.shapes` at all.
    opaque.target = Tref::Ref {
        id: "ext#publisher".into(),
        args: vec![],
    };
    let module = module_of(vec![entry_shape("m#client", vec![plain, opaque])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    for name in ["config", "bus"] {
        let f = entry.fields.iter().find(|f| f.name == name).unwrap();
        assert!(matches!(entry.field_shape(f, &module), FieldShape::Call));
    }
}

fn model_with_a_call_sourced_field() -> crate::ir::Model {
    let config = call_field("config", "ns", "load", vec![]);
    let module = module_of(vec![entry_shape("m#client", vec![config])]);
    crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    }
}

/// Per-target emission of a call source is deferred for every target but Go
/// (its `Emitter` leaf method has no override for TypeScript or Rust yet),
/// so building its resolution plan there would panic. `validate_entries`
/// runs ahead of emission in every gen pipeline and must reject the field
/// there instead, whenever the request generates for a non-Go target, or
/// `tono gen` panics on a spec `tono check` accepted. A request generating
/// for Go only must not trip that same gate.
#[test]
fn gen_rejects_a_call_sourced_field_instead_of_panicking() {
    let model = model_with_a_call_sourced_field();
    let err = super::validate_entries(&model, false).unwrap_err();
    assert!(err.contains("config"), "{err}");
    assert!(err.contains("ext block"), "{err}");
    assert!(super::validate_entries(&model, true).is_ok());
}

/// The same deferral for a call-sourced member of a composed config field.
#[test]
fn gen_rejects_a_call_sourced_config_member_instead_of_panicking() {
    let member = call_field("token", "ns", "load", vec![]);
    let config_shape = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config {
            fields: vec![member],
        },
        traits: vec![],
    };
    let mut conf = field("conf", vec![]);
    conf.target = Tref::Ref {
        id: "m#conf".into(),
        args: vec![],
    };
    let module = module_of(vec![entry_shape("m#client", vec![conf]), config_shape]);
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    let err = super::validate_entries(&model, false).unwrap_err();
    assert!(err.contains("token"), "{err}");
    assert!(err.contains("ext block"), "{err}");
}

fn ext_lib_with_handle(lib: &str, handle: &str) -> ExtLib {
    ExtLib {
        name: lib.into(),
        langs: vec![],
        structs: vec![],
        types: vec![OpaqueType {
            name: handle.into(),
            methods: vec![],
        }],
        externs: vec![],
    }
}

/// An injectable-handle field (decision G, `bus: c.h @with = c.conn(...)`)
/// targets an opaque type declared in the module's own `ext` block. That id
/// lives in `ext_libs`, not `module.shapes`; the module-scoping check must
/// recognize it as same-module instead of misreporting it as an out-of-module
/// reference. The field still stops at the ext-block deferral (the same gate
/// as the plain-call tests above), but for the right reason.
#[test]
fn a_with_and_call_field_targeting_an_ext_block_handle_is_same_module() {
    let mut bus = call_field("bus", "c", "conn", vec![]);
    bus.sources = vec![Source::With];
    bus.target = Tref::Ref {
        id: "c#h".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![bus])]);
    module.ext_libs = vec![ext_lib_with_handle("c", "h")];
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    let err = super::validate_entries(&model, false).unwrap_err();
    assert!(!err.contains("outside this module"), "{err}");
    assert!(err.contains("ext block"), "{err}");
}

/// A plain `@arg`-injected handle field (`bus: c.h @arg`, no call at all)
/// carries no `field.call`, so the call-source gate alone would miss it and
/// let it reach codegen: the target emitter has no idea how to spell a
/// foreign type, and would silently write an import path derived from the
/// `ext` block's own name instead of its declared module path. The gate
/// must reject on the foreign target type too, not only on a call source.
#[test]
fn gen_rejects_a_plain_arg_injected_foreign_handle_field() {
    let mut bus = field("bus", vec![Source::Arg]);
    bus.target = Tref::Ref {
        id: "c#h".into(),
        args: vec![],
    };
    let mut module = module_of(vec![entry_shape("m#client", vec![bus])]);
    module.ext_libs = vec![ext_lib_with_handle("c", "h")];
    let model = crate::ir::Model {
        tono_ir_version: crate::ir::TONO_IR_VERSION,
        modules: vec![module],
    };
    let err = super::validate_entries(&model, false).unwrap_err();
    assert!(err.contains("bus"), "{err}");
    assert!(err.contains("ext block"), "{err}");
}

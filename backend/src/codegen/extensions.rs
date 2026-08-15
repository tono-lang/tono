//! Shared reading of the IR extension table for the target backends. An
//! extension binding is a `ext/{lang}/path#symbol` string; the codegen needs the
//! module path (to import from) and the symbol (to call) split apart, plus a way
//! to find the binding a given target language provides.

use crate::ir::{ExtKind, Model, Module, Shape, ShapeKind, Signature};

/// The four closed lifecycle hook slots, in invocation order.
pub const HOOK_SLOTS: [&str; 4] = [
    "client_init",
    "before_request",
    "after_response",
    "on_error",
];

/// Split a binding reference `ext/ts/sign.ts#signRequest` into its module path
/// and its symbol. Returns `None` when the `#` separator is absent.
pub fn parse_binding(reference: &str) -> Option<(&str, &str)> {
    reference.split_once('#')
}

/// One extension as a specific language binds it: the split module/symbol plus
/// the declared name and kind.
pub struct BoundExtension<'a> {
    pub name: &'a str,
    pub kind: ExtKind,
    pub module: &'a str,
    pub symbol: &'a str,
    pub signature: Option<&'a Signature>,
    /// Impl only: the bound symbol returns a raw outcome for the glue to decode.
    pub raw: bool,
}

/// Every extension in the module that provides a binding for one of `langs`
/// (the target's canonical key plus any alias it accepts, e.g. `["ts",
/// "typescript"]`), in declaration order. The first matching key per extension
/// wins. An extension with no binding for this target emits no call in it.
///
/// A binding whose value is not a "file#symbol" reference is dropped here, but
/// the frontend typecheck rejects that shape (TC0031) before an IR reaches the
/// generator, so a valid document never loses a binding silently.
pub fn bound_extensions<'a>(module: &'a Module, langs: &[&str]) -> Vec<BoundExtension<'a>> {
    module
        .extensions
        .iter()
        .filter_map(|ext| {
            let reference = langs.iter().find_map(|lang| ext.bindings.get(*lang))?;
            let (path, symbol) = parse_binding(reference)?;
            Some(BoundExtension {
                name: &ext.name,
                kind: ext.kind,
                module: path,
                symbol,
                signature: ext.signature.as_ref(),
                raw: ext.raw,
            })
        })
        .collect()
}

/// The binding for a specific lifecycle hook slot, if this language provides one.
pub fn hook_binding<'a>(
    bound: &'a [BoundExtension<'a>],
    slot: &str,
) -> Option<&'a BoundExtension<'a>> {
    bound
        .iter()
        .find(|e| e.kind == ExtKind::Hook && e.name == slot)
}

/// The names an `ext impl` may write to reach the operation with this id. The
/// frontend accepts the bare operation name and, for an entry operation, the
/// qualified `entry.op` form; both reduce to the local part of the shape id.
fn impl_names(op_id: &str) -> Vec<&str> {
    let local = op_id.rsplit('#').next().unwrap_or(op_id);
    match local.split_once('.') {
        Some((_, bare)) => vec![local, bare],
        None => vec![local],
    }
}

/// The impl this language binds for the given operation, if any.
pub fn impl_binding<'a>(
    bound: &'a [BoundExtension<'a>],
    op_id: &str,
) -> Option<&'a BoundExtension<'a>> {
    let names = impl_names(op_id);
    bound
        .iter()
        .find(|e| e.kind == ExtKind::Impl && names.contains(&e.name))
}

/// The operations nested in an entry body. A loose operation is a bare contract
/// that no target emits a body for, so it carries no implementation duty; the
/// frontend scopes its own count the same way.
fn entry_operations(module: &Module) -> Vec<&Shape> {
    module
        .shapes
        .iter()
        .flat_map(|s| match &s.kind {
            ShapeKind::Entry { operations, .. } => operations.iter().collect(),
            _ => Vec::new(),
        })
        .collect()
}

/// The strong bespoke gate. A `kind=contract` extension must carry a
/// conformance reference, or the generator refuses to emit. An `impl` bound in
/// more than one language must be covered by a declared test whose call
/// exercises the operation: nothing else proves the implementations agree, and
/// the whole point of a multi-language binding is that they do. A
/// hook/constraint is lighter and needs neither. An unknown hook slot cannot
/// reach here: `HookSlot` is not part of the wire (the frontend validates the
/// closed lifecycle), and an unrecognized `kind` fails to decode before
/// generation.
pub fn validate_extensions(model: &Model) -> Result<(), String> {
    for module in &model.modules {
        for ext in &module.extensions {
            match ext.kind {
                ExtKind::Contract => {
                    if ext.conformance.is_none() {
                        return Err(format!(
                            "contract extension '{}' in module '{}' requires a conformance reference but has none",
                            ext.name, module.name
                        ));
                    }
                }
                ExtKind::Impl => {
                    if ext.bindings.len() > 1 && !impl_covered_by_test(module, ext) {
                        return Err(format!(
                            "impl extension '{}' in module '{}' is bound in more than one language \
                             but no declared test calls the operation; add a `test` block that \
                             exercises it",
                            ext.name, module.name
                        ));
                    }
                }
                ExtKind::Hook | ExtKind::Constraint => {}
            }
        }
    }
    Ok(())
}

/// Whether any declared test of the module calls the operation the impl
/// binds. An impl may be written against the bare op name or the qualified
/// `entry.op` form; a test call carries the entry (through its construction)
/// and the bare op name.
fn impl_covered_by_test(module: &Module, ext: &crate::ir::Extension) -> bool {
    module.tests.iter().any(|test| {
        test.calls.iter().any(|call| {
            let entry = test
                .constructions
                .iter()
                .find(|c| c.binding == call.client)
                .map(|c| c.entry.as_str());
            let qualified = entry.map(|e| format!("{e}.{}", call.op));
            ext.name == call.op || qualified.as_deref() == Some(ext.name.as_str())
        })
    })
}

/// The implementation count, per target. The frontend proves every entry
/// operation has an implementation; only here is it known which languages are
/// being emitted, so only here can "and one for *this* language" be checked. An
/// operation with no wire binding is bespoke-bound, so it needs a binding for
/// every target in this run or its generated method would have no body.
pub fn validate_impl_coverage(model: &Model, langs: &[&[&str]]) -> Result<(), String> {
    for module in &model.modules {
        for target in langs {
            let bound = bound_extensions(module, target);
            for op in entry_operations(module) {
                if crate::codegen::ops::wire_binding(op).is_some() {
                    continue;
                }
                // An op's own `impl .field.method(args)` body is a
                // third implementation source alongside a protocol binding
                // and the legacy `ext impl` extension: it needs no bound
                // extension at all, the call is direct.
                if crate::codegen::ops::op_impl_call(op).is_some() {
                    continue;
                }
                if impl_binding(&bound, &op.id).is_none() {
                    return Err(format!(
                        "operation '{}' in module '{}' has no protocol binding and no '{}' impl; \
                         bind it with 'ext impl' for every target you generate",
                        op.id,
                        module.name,
                        target.first().copied().unwrap_or("?")
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Extension, Prim, Signature, Tref};

    #[test]
    fn parse_binding_splits_on_the_hash() {
        assert_eq!(
            parse_binding("ext/ts/sign.ts#signRequest"),
            Some(("ext/ts/sign.ts", "signRequest"))
        );
        assert_eq!(parse_binding("no-hash"), None);
    }

    /// A single-module model carrying the given extensions, enough to exercise the
    /// gate (which reads nothing else).
    fn model_with(extensions: Vec<Extension>) -> Model {
        Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![Module {
                tests: vec![],
                name: "m".into(),
                shapes: vec![],
                operations: vec![],
                extensions,
                ext_libs: vec![],
            }],
        }
    }

    fn contract(name: &str, conformance: Option<&str>) -> Extension {
        Extension {
            name: name.into(),
            kind: ExtKind::Contract,
            signature: Some(Signature {
                input: Tref::Prim(Prim::String),
                output: Tref::Prim(Prim::String),
            }),
            raw: false,
            bindings: [("ts".to_string(), "ext/ts/s.ts#sign".to_string())]
                .into_iter()
                .collect(),
            conformance: conformance.map(str::to_string),
        }
    }

    fn binding(name: &str, kind: ExtKind, target: &str) -> Extension {
        Extension {
            name: name.into(),
            kind,
            signature: None,
            raw: false,
            bindings: [("ts".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        }
    }

    /// An impl naming `op_name`, bound in each of `langs`.
    fn impl_ext(op_name: &str, langs: &[&str], conformance: Option<&str>) -> Extension {
        Extension {
            name: op_name.into(),
            kind: ExtKind::Impl,
            signature: None,
            raw: false,
            bindings: langs
                .iter()
                .map(|l| (l.to_string(), format!("ext/{l}/impl#Do")))
                .collect(),
            conformance: conformance.map(str::to_string),
        }
    }

    /// A module with one entry holding one operation of the given id, and the
    /// given extensions. The operation carries no wire descriptor, so it is
    /// bespoke-bound.
    fn entry_model(op_id: &str, extensions: Vec<Extension>) -> Model {
        let op = Shape {
            id: op_id.into(),
            kind: crate::ir::ShapeKind::Operation {
                input: None,
                input_name: None,
                output: None,
                errors: vec![],
                wire: None,
                impl_call: None,
            },
            traits: vec![],
        };
        Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![Module {
                tests: vec![],
                name: "m".into(),
                shapes: vec![Shape {
                    id: "m#client".into(),
                    kind: ShapeKind::Entry {
                        fields: vec![],
                        operations: vec![op],
                    },
                    traits: vec![],
                }],
                operations: vec![],
                extensions,
                ext_libs: vec![],
            }],
        }
    }

    #[test]
    fn an_impl_is_reached_by_its_bare_or_qualified_name() {
        let module = &entry_model("m#client.save", vec![]).modules[0];
        for written in ["save", "client.save"] {
            let exts = vec![impl_ext(written, &["ts"], None)];
            let m = Module {
                extensions: exts,
                ..module.clone()
            };
            let bound = bound_extensions(&m, &["ts"]);
            assert!(
                impl_binding(&bound, "m#client.save").is_some(),
                "'{written}' should reach the operation"
            );
            assert!(impl_binding(&bound, "m#client.other").is_none());
        }
    }

    /// A declared test whose single call exercises `op` on entry `client`.
    fn covering_test(op: &str) -> crate::ir::TestDecl {
        crate::ir::TestDecl {
            name: "covers it".into(),
            constructions: vec![crate::ir::TestConstruction {
                binding: "c".into(),
                entry: "client".into(),
                values: Default::default(),
            }],
            stubs: vec![],
            calls: vec![crate::ir::TestCall {
                binding: "got".into(),
                client: "c".into(),
                op: op.into(),
                input: None,
            }],
            expects: vec![],
        }
    }

    #[test]
    fn a_multi_language_impl_without_a_covering_test_refuses_to_emit() {
        // Two implementations of one operation are only trustworthy if
        // something proves they agree, so the gate demands a declared test
        // that calls the operation. One language is the lighter case and
        // needs none.
        let one = entry_model("m#client.save", vec![impl_ext("save", &["ts"], None)]);
        assert!(validate_extensions(&one).is_ok());
        let two = entry_model("m#client.save", vec![impl_ext("save", &["ts", "go"], None)]);
        let err = validate_extensions(&two).unwrap_err();
        assert!(err.contains("bound in more than one language"), "{err}");
        assert!(err.contains("test"), "{err}");
        // A conformance reference no longer stands in for the test.
        let vectored = entry_model(
            "m#client.save",
            vec![impl_ext("save", &["ts", "go"], Some("vectors/save.json"))],
        );
        assert!(validate_extensions(&vectored).is_err());
        // A test calling the op satisfies the gate, for the bare and the
        // qualified impl spelling alike.
        for written in ["save", "client.save"] {
            let mut covered = entry_model(
                "m#client.save",
                vec![impl_ext(written, &["ts", "go"], None)],
            );
            covered.modules[0].tests = vec![covering_test("save")];
            assert!(
                validate_extensions(&covered).is_ok(),
                "'{written}' should be covered"
            );
        }
        // A test calling some other op does not.
        let mut miscovered =
            entry_model("m#client.save", vec![impl_ext("save", &["ts", "go"], None)]);
        miscovered.modules[0].tests = vec![covering_test("other")];
        assert!(validate_extensions(&miscovered).is_err());
    }

    #[test]
    fn the_coverage_gate_counts_one_implementation_per_target() {
        let ts = &["ts", "typescript"] as &[&str];
        let go = &["go"] as &[&str];
        let model = entry_model("m#client.save", vec![impl_ext("save", &["ts"], None)]);
        assert!(validate_impl_coverage(&model, &[ts]).is_ok());
        // Generating Go too needs a Go binding: the method would have no body.
        let err = validate_impl_coverage(&model, &[ts, go]).unwrap_err();
        assert!(err.contains("m#client.save"), "{err}");
        assert!(err.contains("'go' impl"), "{err}");
    }

    #[test]
    fn an_operation_with_a_descriptor_needs_no_impl() {
        let mut model = entry_model("m#client.save", vec![]);
        if let ShapeKind::Entry { operations, .. } = &mut model.modules[0].shapes[0].kind {
            if let ShapeKind::Operation { wire, .. } = &mut operations[0].kind {
                *wire = Some(crate::codegen::test_support::wire_binding("GET"));
            }
        }
        assert!(validate_impl_coverage(&model, &[&["go"]]).is_ok());
    }

    #[test]
    fn a_contract_without_conformance_refuses_to_emit() {
        // The strong bespoke gate: a contract with a conformance reference passes;
        // dropping the reference fails the gate.
        assert!(validate_extensions(&model_with(vec![contract(
            "sign",
            Some("vectors/sign.json")
        )]))
        .is_ok());
        assert!(validate_extensions(&model_with(vec![contract("sign", None)])).is_err());
    }

    #[test]
    fn a_hook_or_constraint_without_conformance_passes() {
        // Hooks and constraints are lighter; a conformance reference is optional.
        let model = model_with(vec![
            binding("before_request", ExtKind::Hook, "ext/ts/a.ts#f"),
            binding("luhn", ExtKind::Constraint, "ext/ts/l.ts#h"),
        ]);
        assert!(validate_extensions(&model).is_ok());
    }
}

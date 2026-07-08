//! Shared reading of the IR extension table for the target backends. An
//! extension binding is a `ext/{lang}/path#symbol` string; the codegen needs the
//! module path (to import from) and the symbol (to call) split apart, plus a way
//! to find the binding a given target language provides.

use crate::ir::{ExtKind, Model, Module, Signature};

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

/// The strong bespoke gate: a `kind=contract` extension must carry a conformance
/// reference, or the generator refuses to emit. A hook/constraint is lighter and
/// needs none. An unknown hook slot cannot reach here: `HookSlot` is not part of
/// the wire (the frontend validates the closed lifecycle), and an unrecognized
/// `kind` fails to decode before generation.
pub fn validate_extensions(model: &Model) -> Result<(), String> {
    for module in &model.modules {
        for ext in &module.extensions {
            if ext.kind == ExtKind::Contract && ext.conformance.is_none() {
                return Err(format!(
                    "contract extension '{}' in module '{}' requires a conformance reference but has none",
                    ext.name, module.name
                ));
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
                name: "m".into(),
                shapes: vec![],
                operations: vec![],
                extensions,
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
            bindings: [("ts".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        }
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

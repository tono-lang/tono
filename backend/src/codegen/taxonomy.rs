//! Which error-taxonomy categories a module's generated code can actually
//! construct, for a given target.
//!
//! Every module gets the full six-category taxonomy (`Violation`/
//! `ValidationError`, `TransportError`, `DecodeError`, `ContractError`,
//! `APIError`/`APIFailure`, `ConfigError`) baked in today regardless of
//! whether any generated code path produces each one, which is dead surface
//! in the delivered SDK. This module answers, per `(module, target
//! languages)`, which categories a generated code path can actually reach,
//! so a target's `errors.rs` can build only the live ones and the client/
//! entry glue that constructs `ContractError` can skip the wrap where no
//! bespoke boundary exists to need it.
//!
//! Deliberately pure over the IR (no codegen run): that is what lets
//! `compat.rs` reuse the same predicates to detect a category disappearing
//! between two versions without generating source, mirroring how
//! [`crate::codegen::visibility::Exposed`] is derived once from the model and
//! threaded through every target's emission.

use crate::codegen::entries::{has_entries, module_entries};
use crate::codegen::extensions::{bound_extensions, impl_binding};
use crate::codegen::ops::{module_declared_errors, wire_descriptor};
use crate::codegen::validation::shape_has_checks;
use crate::ir::Module;

/// Whether a module's generated code can construct each error-taxonomy
/// category, for one target. Computed once per `(module, target)` and
/// threaded through emission, the same shape as [`crate::codegen::visibility::Exposed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaxonomyLiveness {
    pub validation: bool,
    pub transport: bool,
    pub decode: bool,
    pub api: bool,
    pub config: bool,
    pub contract: bool,
}

impl TaxonomyLiveness {
    /// Every category live: the escape hatch for a target whose loose
    /// (non-entry) operation surface is a trait/interface only (Rust, Go),
    /// never a concrete client. Nothing in the generated SDK constructs any
    /// category there either way, so the full taxonomy is vocabulary the
    /// trait implementer may need, not dead code a prune should remove.
    pub fn all_live() -> Self {
        Self {
            validation: true,
            transport: true,
            decode: true,
            api: true,
            config: true,
            contract: true,
        }
    }
}

/// Derive liveness for a module against one target's binding languages (as
/// [`bound_extensions`] takes them, e.g. `["rust"]` or `["ts", "typescript"]`).
/// Meant for a module whose operations get a concrete client: every target's
/// entries, and TypeScript's loose operations (its `HttpClient`).
///
/// `transport`/`decode` are scoped to *wire* operations specifically, not
/// every operation: a bespoke (`ext impl`-bound) loose or entry operation
/// never goes through the runtime's HTTP call, so it never produces a
/// transport failure or a decoded response to classify, only (conditionally)
/// a `ContractError` — see [`contract_live`]. A module made only of bespoke
/// loose operations (TypeScript's `HttpClient` skips it entirely; Rust/Go
/// emit only a trait/interface for it, handled by [`TaxonomyLiveness::all_live`]
/// instead of this function) has no wire call site either.
///
/// `api` is broader: it is also live whenever the module declares *any*
/// operation error at all (wire or not), since the discriminator function and
/// the declared-error classes are generated per declared error regardless of
/// how the operation is bound, and both need the `Api` category (the
/// `Undeclared` fallback, the declared-error base class) to exist.
pub fn derive(module: &Module, langs: &[&str]) -> TaxonomyLiveness {
    let has_wire_ops = module
        .operations
        .iter()
        .any(|op| wire_descriptor(op).is_some())
        || has_wire_entry_op(module);
    TaxonomyLiveness {
        validation: module.shapes.iter().any(shape_has_checks),
        transport: has_wire_ops,
        decode: has_wire_ops,
        api: has_wire_ops || !module_declared_errors(module).is_empty(),
        config: has_entries(module),
        contract: contract_live(module, langs),
    }
}

/// Whether the module can construct a `ContractError` for this target: it
/// binds a hook or a typed contract/constraint/impl, or an entry has a
/// bespoke (non-wire) operation this target has no binding for at all, which
/// always falls back to the "operation has no implementation" `ContractError`
/// regardless of whether anything else is bound.
fn contract_live(module: &Module, langs: &[&str]) -> bool {
    let bound = bound_extensions(module, langs);
    if !bound.is_empty() {
        return true;
    }
    module_entries(module).iter().any(|entry| {
        entry
            .operations
            .iter()
            .any(|op| wire_descriptor(op).is_none() && impl_binding(&bound, &op.id).is_none())
    })
}

/// An entry module's taxonomy liveness for Rust: [`derive`]'s general
/// predicate, widened for `Contract` because every wire-op entry method's
/// generated body (`rust/entry/mod.rs::op_method`) contains a
/// `Runtime::execute(...).map_err(...)` match arm that wraps any downcast
/// failure into `ContractError`. That text is emitted the same way for every
/// wire-op entry method regardless of whether *this* module binds a hook for
/// it — only whether the arm is reachable at runtime depends on the binding
/// (the runtime's `hooks` field is `Some` only when a `before_request`/
/// `after_response` hook is bound); the reference itself is unconditional.
/// See [`has_wire_entry_op`]. Shared by the Rust emitter and `compat.rs` (a
/// pruned-declaration breaking change has to apply the same rule the emitter
/// does, or it would flag a category as newly-dead that Rust still emits
/// unconditionally).
pub fn derive_rust_entry(module: &Module, langs: &[&str]) -> TaxonomyLiveness {
    let base = derive(module, langs);
    TaxonomyLiveness {
        contract: base.contract || has_wire_entry_op(module),
        ..base
    }
}

/// Whether any entry in the module has at least one wire-descriptor
/// (`@http`-bound) operation: the module has a real HTTP call site, as
/// opposed to entries made only of bespoke (`ext impl`-bound) operations.
pub fn has_wire_entry_op(module: &Module) -> bool {
    module_entries(module).iter().any(|entry| {
        entry
            .operations
            .iter()
            .any(|op| wire_descriptor(op).is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{member_constrained, structure};
    use crate::ir::{EntryField, ExtKind, Extension, Prim, Shape, ShapeKind, Signature, Tref};

    fn module(shapes: Vec<Shape>, operations: Vec<Shape>, extensions: Vec<Extension>) -> Module {
        Module {
            tests: vec![],
            name: "m".into(),
            shapes,
            operations,
            extensions,
        }
    }

    fn wire_op(id: &str) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
                wire: None,
            },
            traits: vec![crate::ir::Trait {
                id: "wire_descriptor".into(),
                value: serde_json::json!({}),
            }],
        }
    }

    fn bespoke_op(id: &str) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![],
                wire: None,
            },
            traits: vec![],
        }
    }

    fn entry(id: &str, operations: Vec<Shape>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Entry {
                fields: vec![EntryField {
                    name: "endpoint".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![],
                    format: None,
                    transforms: vec![],
                    select: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                }],
                operations,
            },
            traits: vec![],
        }
    }

    fn contract_ext(target: &str) -> Extension {
        Extension {
            name: "sign".into(),
            kind: ExtKind::Contract,
            signature: Some(Signature {
                input: Tref::Prim(Prim::String),
                output: Tref::Prim(Prim::String),
            }),
            raw: false,
            bindings: [("rust".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: Some("v.json".into()),
        }
    }

    #[test]
    fn a_module_with_no_shapes_or_operations_has_nothing_live() {
        let liveness = derive(&module(vec![], vec![], vec![]), &["rust"]);
        assert!(!liveness.validation);
        assert!(!liveness.transport);
        assert!(!liveness.decode);
        assert!(!liveness.api);
        assert!(!liveness.config);
        assert!(!liveness.contract);
    }

    #[test]
    fn constraints_alone_light_up_only_validation() {
        let checked = structure(
            "m#Account",
            vec![member_constrained(
                "id",
                Tref::Prim(Prim::I64),
                vec![crate::ir::Constraint::Range {
                    min: Some(0.0),
                    max: None,
                    excl_min: false,
                    excl_max: false,
                }],
            )],
        );
        let liveness = derive(&module(vec![checked], vec![], vec![]), &["rust"]);
        assert!(liveness.validation);
        assert!(!liveness.transport);
        assert!(!liveness.decode);
        assert!(!liveness.api);
        assert!(!liveness.config);
        assert!(!liveness.contract);
    }

    #[test]
    fn a_loose_wire_operation_lights_up_transport_decode_api_but_not_config_or_contract() {
        let liveness = derive(
            &module(vec![], vec![wire_op("m#do_thing")], vec![]),
            &["rust"],
        );
        assert!(!liveness.validation);
        assert!(liveness.transport);
        assert!(liveness.decode);
        assert!(liveness.api);
        assert!(!liveness.config);
        assert!(!liveness.contract);
    }

    #[test]
    fn a_loose_bespoke_operation_alone_has_no_live_transport_decode_or_api() {
        // No wire call site: a purely local operation never goes through the
        // runtime's HTTP call.
        let liveness = derive(
            &module(vec![], vec![bespoke_op("m#do_thing")], vec![]),
            &["rust"],
        );
        assert!(!liveness.transport);
        assert!(!liveness.decode);
        assert!(!liveness.api);
    }

    #[test]
    fn a_declared_error_lights_up_api_even_with_no_wire_call_site() {
        // The discriminator function and the declared-error class are
        // generated per declared error regardless of how the operation is
        // bound, so `Api` (the fallback and the declared-error base) is live
        // whether or not the operation ever touches the runtime's HTTP call.
        let op = Shape {
            id: "m#do_thing".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: vec![Tref::Ref {
                    id: "m#bad".into(),
                    args: vec![],
                }],
                wire: None,
            },
            traits: vec![],
        };
        let bad = crate::codegen::test_support::error_shape("m#bad", vec![], 400, None, false);
        let liveness = derive(&module(vec![bad], vec![op], vec![]), &["rust"]);
        assert!(!liveness.transport);
        assert!(!liveness.decode);
        assert!(liveness.api);
    }

    #[test]
    fn an_entry_lights_up_config_even_with_no_bound_extension() {
        let liveness = derive(
            &module(
                vec![entry("m#client", vec![wire_op("m#client.ping")])],
                vec![],
                vec![],
            ),
            &["rust"],
        );
        assert!(liveness.transport);
        assert!(liveness.config);
        assert!(!liveness.contract);
    }

    #[test]
    fn a_bound_contract_lights_up_contract_for_the_bound_target_only() {
        let m = module(
            vec![entry("m#client", vec![wire_op("m#client.ping")])],
            vec![],
            vec![contract_ext("ext/rust/sign.rs#sign")],
        );
        assert!(derive(&m, &["rust"]).contract);
        assert!(!derive(&m, &["typescript"]).contract);
    }

    #[test]
    fn an_entry_with_an_unbound_bespoke_op_lights_up_contract() {
        // No wire descriptor and no impl binding: the "operation has no
        // implementation" fallback always constructs a ContractError.
        let m = module(
            vec![entry("m#client", vec![bespoke_op("m#client.custom")])],
            vec![],
            vec![],
        );
        assert!(derive(&m, &["rust"]).contract);
    }

    #[test]
    fn an_entry_whose_only_op_is_wire_bound_and_unextended_has_no_live_contract() {
        let m = module(
            vec![entry("m#client", vec![wire_op("m#client.ping")])],
            vec![],
            vec![],
        );
        assert!(!derive(&m, &["rust"]).contract);
    }

    #[test]
    fn has_wire_entry_op_is_true_only_when_an_entry_op_carries_a_wire_descriptor() {
        let wired = module(
            vec![entry("m#client", vec![wire_op("m#client.ping")])],
            vec![],
            vec![],
        );
        assert!(has_wire_entry_op(&wired));

        let bespoke_only = module(
            vec![entry("m#client", vec![bespoke_op("m#client.custom")])],
            vec![],
            vec![],
        );
        assert!(!has_wire_entry_op(&bespoke_only));
    }

    #[test]
    fn a_bespoke_only_entry_has_no_live_transport_decode_or_api() {
        let m = module(
            vec![entry("m#client", vec![bespoke_op("m#client.custom")])],
            vec![],
            vec![],
        );
        let liveness = derive(&m, &["rust"]);
        assert!(!liveness.transport);
        assert!(!liveness.decode);
        assert!(!liveness.api);
        // The entry itself still goes through construction.
        assert!(liveness.config);
    }

    #[test]
    fn all_live_reports_every_category() {
        let liveness = TaxonomyLiveness::all_live();
        assert!(liveness.validation);
        assert!(liveness.transport);
        assert!(liveness.decode);
        assert!(liveness.api);
        assert!(liveness.config);
        assert!(liveness.contract);
    }
}

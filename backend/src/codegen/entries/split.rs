//! How an entry's construction splits around its foreign calls.
//!
//! A field constructed through an `ext` library (`= ns.fn(args)` or
//! `= .handle.method(args)`) is resolved by a named function of its own,
//! taking exactly the sibling fields the call reads and returning the
//! library's value. Everything the constructor can resolve without a foreign
//! call (the `@arg` values, the env chains, the defaults, the derivations
//! over those) is resolved first, in one shared step, so a generated test
//! can build the same settings and then put its fakes where the foreign
//! values would go, without a runtime branch deciding between the two.
//!
//! The split is target-independent: which fields the shared step resolves,
//! which are foreign, which non-foreign fields depend on a foreign one and so
//! must follow it, and which handles never enter the settings at all because
//! their only reader is another construction call. Each target spells the
//! resulting sequence in its own idiom.

use std::collections::HashSet;

use crate::ir::{EntryField, Model, Module, ShapeKind};

use super::order::{call_arg_heads, dependencies};
use super::validate_ownership::forwarded_handles;
use super::{has_entries, EntryModel};

/// One step of the constructor after the shared settings resolved, in
/// resolution order.
pub enum TailStep<'a> {
    /// A field with its own `= ns.fn(args)` construction call, resolved by
    /// the library's function.
    Call(&'a EntryField),
    /// A field sourced from a method of an already-resolved handle,
    /// resolved by the same function over the real handle or a test's fake.
    HandleCall(&'a EntryField),
    /// A field with no foreign call of its own that reads one (directly or
    /// through another such field), so it can only resolve after it.
    Dependent(&'a EntryField),
}

impl<'a> TailStep<'a> {
    pub fn field(&self) -> &'a EntryField {
        match self {
            TailStep::Call(f) | TailStep::HandleCall(f) | TailStep::Dependent(f) => f,
        }
    }
}

/// The construction sequence of one entry: the fields the shared settings
/// step resolves, then the steps that follow it, both in resolution order.
pub struct ConstructionSplit<'a> {
    pub settings: Vec<&'a EntryField>,
    pub tail: Vec<TailStep<'a>>,
}

/// Whether a field is constructed through a foreign call.
pub fn is_foreign(field: &EntryField) -> bool {
    field.call.is_some() || field.handle_call.is_some()
}

/// The sibling-field heads a foreign call reads, in first-appearance order
/// and without repeats: a handle method call's receiver first, then every
/// `Ref` head among the arguments. These are the parameters the field's
/// resolver takes, so a dependency the spec did not write cannot reach it.
pub fn call_deps(field: &EntryField) -> Vec<String> {
    let mut heads: Vec<&str> = Vec::new();
    if let Some(call) = &field.handle_call {
        heads.extend(call.recv.first().map(String::as_str));
        for arg in &call.args {
            call_arg_heads(arg, &mut heads);
        }
    }
    if let Some(call) = &field.call {
        for arg in &call.args {
            call_arg_heads(arg, &mut heads);
        }
    }
    let mut out: Vec<String> = Vec::new();
    for head in heads {
        if !out.iter().any(|h| h == head) {
            out.push(head.to_string());
        }
    }
    out
}

impl<'a> EntryModel<'a> {
    /// The foreign handle fields forwarded by value into another field's
    /// construction call. Such a handle is owned by the call that received
    /// it (`validate_ownership` refuses any other reader), so it never enters
    /// the settings: the constructor hands the resolved value straight to
    /// the consuming resolver.
    pub fn forwarded_handles(&self, module: &Module) -> Vec<&'a EntryField> {
        let declared = self.declared();
        let mut out: Vec<&'a EntryField> = Vec::new();
        for owner in &declared {
            for handle in forwarded_handles(module, &declared, owner) {
                if !out.iter().any(|f| f.name == handle.name) {
                    out.push(handle);
                }
            }
        }
        out
    }

    /// Whether the field with this name is a forwarded handle (see
    /// [`Self::forwarded_handles`]).
    pub fn is_forwarded(&self, module: &Module, name: &str) -> bool {
        self.forwarded_handles(module)
            .iter()
            .any(|f| f.name == name)
    }

    /// The fields the settings hold, in declaration order: every declared
    /// field except a forwarded handle.
    pub fn stored(&self, module: &Module) -> Vec<&'a EntryField> {
        let forwarded = self.forwarded_handles(module);
        self.declared()
            .into_iter()
            .filter(|f| !forwarded.iter().any(|h| h.name == f.name))
            .collect()
    }

    /// The construction sequence: a field goes to the shared settings step
    /// unless it is foreign or reads (directly or transitively) a field that
    /// is. Both halves keep the resolution order among themselves, and a
    /// field only ever follows what it reads, so hoisting the shared step
    /// ahead of the foreign calls changes no dependency.
    pub fn construction_split(&self, module: &'a Module) -> ConstructionSplit<'a> {
        let mut settings = Vec::new();
        let mut tail = Vec::new();
        let mut trailing: HashSet<&str> = HashSet::new();
        for field in &self.fields {
            let reads_trailing = dependencies(field, module)
                .into_iter()
                .any(|dep| trailing.contains(dep));
            let step = if field.call.is_some() {
                Some(TailStep::Call(field))
            } else if field.handle_call.is_some() {
                Some(TailStep::HandleCall(field))
            } else if reads_trailing {
                Some(TailStep::Dependent(field))
            } else {
                None
            };
            match step {
                Some(step) => {
                    trailing.insert(field.name.as_str());
                    tail.push(step);
                }
                None => settings.push(*field),
            }
        }
        ConstructionSplit { settings, tail }
    }
}

/// Whether any operation of the module goes over the wire. Without one the
/// SDK has no transport at all: no transport slots on the settings, no
/// exclusivity check, no transport helpers, and no transport dependencies.
pub fn has_wire_ops(module: &Module) -> bool {
    let entry_ops = module.shapes.iter().flat_map(|s| match &s.kind {
        ShapeKind::Entry { operations, .. } => operations.as_slice(),
        _ => &[],
    });
    entry_ops
        .chain(module.operations.iter())
        .any(|op| matches!(&op.kind, ShapeKind::Operation { wire: Some(_), .. }))
}

/// Whether the model's entries go over the wire anywhere (the SDK-wide
/// question the shared transport groups and the native manifests ask).
pub fn model_has_wire_ops(model: &Model) -> bool {
    model
        .modules
        .iter()
        .any(|m| has_entries(m) && has_wire_ops(m))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::entries::module_entries;
    use crate::codegen::targets::go::entry::ext_fixtures::{
        composed_handles_module, reference_example_module,
    };
    use crate::ir::{EntryCall, EnvName, Source, TemplatePart, Tref};

    fn names<'a>(fields: impl IntoIterator<Item = &'a EntryField>) -> Vec<&'a str> {
        fields.into_iter().map(|f| f.name.as_str()).collect()
    }

    fn step_names<'a>(steps: &[TailStep<'a>]) -> Vec<(&'static str, &'a str)> {
        steps
            .iter()
            .map(|s| match s {
                TailStep::Call(f) => ("call", f.name.as_str()),
                TailStep::HandleCall(f) => ("handle_call", f.name.as_str()),
                TailStep::Dependent(f) => ("dependent", f.name.as_str()),
            })
            .collect()
    }

    #[test]
    fn a_handle_forwarded_into_another_call_leaves_the_settings() {
        let module = composed_handles_module();
        let entries = module_entries(&module);
        let entry = &entries[0];
        // `primary` and `secondary` are handed to `new_combined` by value;
        // `combined` is only read by an op and `injected` only by an op, so
        // both stay stored.
        assert_eq!(
            names(entry.forwarded_handles(&module)),
            vec!["primary", "secondary"]
        );
        assert!(entry.is_forwarded(&module, "primary"));
        assert!(!entry.is_forwarded(&module, "combined"));
        assert_eq!(
            names(entry.stored(&module)),
            vec!["a", "b", "c", "d", "injected", "combined"]
        );
    }

    #[test]
    fn the_shared_step_takes_every_field_no_foreign_call_reaches() {
        let module = composed_handles_module();
        let entries = module_entries(&module);
        let split = entries[0].construction_split(&module);
        assert_eq!(names(split.settings), vec!["a", "b", "c", "d", "injected"]);
        assert_eq!(
            step_names(&split.tail),
            vec![
                ("call", "primary"),
                ("call", "secondary"),
                ("call", "combined")
            ]
        );
    }

    #[test]
    fn a_field_reading_a_foreign_value_follows_it_and_its_readers_follow_too() {
        let mut module = reference_example_module();
        // `bus` reads `config` (a call field) through a member path; add a
        // derivation over `config` and a plain field derived from that one,
        // so the transitive rule is exercised on both a foreign head and a
        // dependent head.
        let ShapeKind::Entry { fields, .. } = &mut module.shapes[4].kind else {
            panic!("entry");
        };
        let mut auth = crate::codegen::targets::go::entry::ext_fixtures::field(
            "auth",
            Tref::Prim(crate::ir::Prim::String),
            vec![],
        );
        auth.format = Some(vec![
            TemplatePart::Lit("Bearer ".into()),
            TemplatePart::Field(vec!["config".into(), "token".into()]),
        ]);
        let label = crate::codegen::targets::go::entry::ext_fixtures::field(
            "label",
            Tref::Prim(crate::ir::Prim::String),
            vec![Source::Env(EnvName::Field(crate::ir::FieldRef {
                field: vec!["auth".into()],
            }))],
        );
        // An independent field declared last still resolves in the shared
        // step: nothing foreign reaches it.
        let plain = crate::codegen::targets::go::entry::ext_fixtures::field(
            "plain",
            Tref::Prim(crate::ir::Prim::String),
            vec![Source::Arg],
        );
        fields.extend([auth, label, plain]);
        let entries = module_entries(&module);
        let split = entries[0].construction_split(&module);
        assert_eq!(names(split.settings), vec!["service", "region", "plain"]);
        assert_eq!(
            step_names(&split.tail),
            vec![
                ("call", "config"),
                ("call", "bus"),
                ("dependent", "auth"),
                ("dependent", "label"),
            ]
        );
    }

    #[test]
    fn a_resolver_takes_the_heads_its_call_reads_once_each_receiver_first() {
        let module = reference_example_module();
        let entries = module_entries(&module);
        let bus = entries[0]
            .fields
            .iter()
            .find(|f| f.name == "bus")
            .expect("bus");
        // Two member paths into the same head are one parameter.
        assert_eq!(call_deps(bus), vec!["config"]);
        let mut sourced = crate::codegen::targets::go::entry::ext_fixtures::field(
            "sourced",
            Tref::Prim(crate::ir::Prim::String),
            vec![],
        );
        sourced.handle_call = Some(crate::ir::OpImplCall {
            recv: vec!["bus".into()],
            method: "send".into(),
            args: vec![
                crate::codegen::targets::go::entry::ext_fixtures::call_ref(&["region"]),
                crate::codegen::targets::go::entry::ext_fixtures::call_ref(&["bus"]),
            ],
        });
        assert_eq!(call_deps(&sourced), vec!["bus", "region"]);
        let mut literal_only = sourced.clone();
        literal_only.handle_call = None;
        literal_only.call = Some(EntryCall {
            ns: "companybus".into(),
            func: "connect".into(),
            args: vec![crate::ir::CallArg::Lit(serde_json::json!("x"))],
        });
        assert!(call_deps(&literal_only).is_empty());
    }

    #[test]
    fn a_module_without_a_wire_operation_has_no_transport() {
        let module = reference_example_module();
        assert!(!has_wire_ops(&module));
        let with_wire = crate::codegen::fixtures::handle_source::handle_source_module("go");
        assert!(has_wire_ops(&with_wire));
        let model = Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![module, with_wire],
        };
        assert!(model_has_wire_ops(&model));
        let ext_only = Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![reference_example_module()],
        };
        assert!(!model_has_wire_ops(&ext_only));
    }
}

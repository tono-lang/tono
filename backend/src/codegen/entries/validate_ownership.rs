//! Generation-time ownership rule for a foreign handle handed on to another
//! extern call: it has exactly one reader.
//!
//! A handle field forwarded as another field's construction-call argument
//! (`relay = lib.attach(.bus, ..)`) gives the callee the value itself. In
//! Rust that is a move out of the field's slot: a foreign handle is not
//! `Clone` in general (a connection, a pool, a provider), a by-value
//! parameter cannot be satisfied through an `Rc`/`Arc` the callee's
//! signature does not name, and the slot is left unset once ownership went
//! to the callee. Anything else in the entry that reads the same handle
//! afterwards (an op's own `impl .bus.method(..)`, a second forwarding
//! call) would compile and then diagnose "not configured" at runtime for a
//! handle the caller did configure. A target with reference semantics
//! (Go, TypeScript) would happily alias it instead, so the same spec would
//! mean two different things by target.
//!
//! The rule is therefore enforced once, target-agnostically: a spec's
//! construction semantics must not depend on the target, and single
//! ownership is the tightest model every target can honour. Rejected here,
//! naming the field and both readers, rather than in one target's emitter.

use super::EntryModel;
use crate::ir::{EntryField, Module, ShapeKind};

use super::validate::{is_foreign_ref, ref_id, ref_paths};

/// One place a handle field is read from, spelled for the diagnostic.
fn call_site(field: &EntryField) -> String {
    let call = field
        .call
        .as_ref()
        .expect("only a call field forwards a handle");
    format!("{} = {}.{}(..)", field.name, call.ns, call.func)
}

/// The sibling handle fields a call field forwards by value (`Ref` head
/// naming a foreign-handle field), each paired with the forwarding call.
fn forwarded_handles<'a>(
    module: &Module,
    declared: &[&'a EntryField],
    field: &'a EntryField,
) -> Vec<&'a EntryField> {
    let Some(call) = &field.call else {
        return Vec::new();
    };
    let mut paths = Vec::new();
    ref_paths(&call.args, &mut paths);
    paths
        .iter()
        .filter_map(|path| path.first())
        .filter_map(|head| declared.iter().copied().find(|f| f.name == *head))
        .filter(|f| ref_id(&f.target).is_some_and(|id| is_foreign_ref(module, id)))
        .collect()
}

/// Every reader of `handle` other than `owner`'s own forwarding call, each
/// spelled for the diagnostic: another field's forwarding call, or an op's
/// own `impl` call reading it as receiver or argument.
fn other_readers(
    module: &Module,
    entry: &EntryModel<'_>,
    declared: &[&EntryField],
    handle: &EntryField,
    owner: &EntryField,
) -> Vec<String> {
    let mut readers = Vec::new();
    for field in declared.iter().copied() {
        if field.name != owner.name
            && forwarded_handles(module, declared, field)
                .iter()
                .any(|h| h.name == handle.name)
        {
            readers.push(call_site(field));
        }
    }
    for op in entry.operations {
        let ShapeKind::Operation {
            impl_call: Some(call),
            ..
        } = &op.kind
        else {
            continue;
        };
        let op_name = super::op_local_name(&op.id);
        if call.recv.first() == Some(&handle.name) {
            readers.push(format!(
                "operation {op_name} impl .{}.{}(..)",
                call.recv.join("."),
                call.method
            ));
        }
        let mut paths = Vec::new();
        ref_paths(&call.args, &mut paths);
        if paths.iter().any(|p| p.first() == Some(&handle.name)) {
            readers.push(format!(
                "operation {op_name} impl .{}.{}(.{})",
                call.recv.join("."),
                call.method,
                handle.name
            ));
        }
    }
    readers
}

/// Rejects a foreign handle field forwarded into another field's call while
/// something else in the entry also reads it. Returns the first offense.
pub(super) fn forwarded_handle_has_one_reader(
    module: &Module,
    entry: &EntryModel<'_>,
) -> Result<(), String> {
    let declared = entry.declared();
    for owner in declared.iter().copied() {
        for handle in forwarded_handles(module, &declared, owner) {
            let readers = other_readers(module, entry, &declared, handle, owner);
            if let Some(reader) = readers.first() {
                return Err(format!(
                    "{}: argument .{} hands the foreign handle {} on to that call, but {} also reads it; a handle forwarded into another call is owned by that call and cannot be read anywhere else (move the second read behind the constructed handle, or construct the handle twice)",
                    call_site(owner),
                    handle.name,
                    handle.name,
                    reader
                ));
            }
        }
    }
    Ok(())
}

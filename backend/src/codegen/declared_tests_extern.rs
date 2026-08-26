//! Extern-call reachability and coverage: which `extern` calls a test's
//! construction/call path can reach (a free-fn call or a handle-method call
//! sourced on an entry field, or an opaque handle's method reached through
//! an op's own `impl` body), and validating a declared `extern_stubs` entry
//! against them.
//! Split out of `declared_tests` to keep it under the file-size ceiling;
//! `use super::*` reaches the parent's IR imports and helpers.

use super::*;
use crate::ir::{EntryField, OpImplCall};

/// Defensive validation of an extern stub's answers: the frontend's own
/// typecheck already matches a stub's value against the extern's declared
/// return/error shapes (the same trust level [`validate_stub`] places in the
/// frontend for `Http`/`Impl` stubs), so this only checks the target
/// resolves against the module's own `ext_libs` and the answer's coarse
/// shape fits the target kind.
pub(super) fn validate_extern_stub(module: &Module, stub: &ExternStub) -> Result<(), String> {
    let (lib_name, display) = match &stub.target {
        ExternStubTarget::Free { lib, fn_ } => (lib.as_str(), format!("{lib}.{fn_}")),
        ExternStubTarget::Method { lib, ty, method } => {
            (lib.as_str(), format!("{lib}.{ty}.{method}"))
        }
    };
    if stub.answers.is_empty() {
        return Err(format!("the extern stub on '{display}' carries no answers"));
    }
    let lib = module
        .ext_libs
        .iter()
        .find(|l| l.name == lib_name)
        .ok_or_else(|| format!("stubs '{display}', which names no declared ext lib"))?;
    match &stub.target {
        ExternStubTarget::Free { fn_, .. } => {
            if !lib.externs.iter().any(|e| e.name == *fn_) {
                return Err(format!(
                    "stubs '{display}', which is not a free function of ext '{lib_name}'"
                ));
            }
            for answer in &stub.answers {
                if !matches!(answer, StubAnswer::Value { .. }) {
                    return Err(format!(
                        "an answer of the extern stub on '{display}' is not a plain value"
                    ));
                }
            }
        }
        ExternStubTarget::Method { ty, method, .. } => {
            let handle = lib.types.iter().find(|t| t.name == *ty).ok_or_else(|| {
                format!("stubs '{display}', but '{lib_name}' declares no opaque type '{ty}'")
            })?;
            if !handle.methods.iter().any(|m| m.name == *method) {
                return Err(format!(
                    "stubs '{display}', which is not a method of '{lib_name}.{ty}'"
                ));
            }
            for answer in &stub.answers {
                if !matches!(answer, StubAnswer::Value { .. } | StubAnswer::Error { .. }) {
                    return Err(format!(
                        "an answer of the extern stub on '{display}' is not a value or a declared error"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// One `extern` call reachable from a test's construction/call path: a free
/// function sourced on an entry field, or an opaque handle's method reached
/// through an op's own `impl` body.
#[derive(Debug, Clone, Copy)]
enum ReachableExtern<'a> {
    Free {
        lib: &'a str,
        fn_: &'a str,
    },
    Method {
        lib: &'a str,
        ty: &'a str,
        method: &'a str,
    },
}

impl std::fmt::Display for ReachableExtern<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReachableExtern::Free { lib, fn_ } => write!(f, "{lib}.{fn_}"),
            ReachableExtern::Method { lib, ty, method } => write!(f, "{lib}.{ty}.{method}"),
        }
    }
}

impl ReachableExtern<'_> {
    fn matches(&self, target: &ExternStubTarget) -> bool {
        match (self, target) {
            (ReachableExtern::Free { lib, fn_ }, ExternStubTarget::Free { lib: l, fn_: f }) => {
                lib == l && fn_ == f
            }
            (
                ReachableExtern::Method { lib, ty, method },
                ExternStubTarget::Method {
                    lib: l,
                    ty: t,
                    method: m,
                },
            ) => lib == l && ty == t && method == m,
            _ => false,
        }
    }
}

/// The foreign opaque handle a field's declared type names in the module's
/// own `ext_libs` (`"{lib}#{type}"`), when it does. Mirrors the Go
/// emitter's own `foreign_handle` (`targets/go/entry/ext.rs`), kept
/// independent here since the planner runs before any target is chosen.
fn foreign_handle_type<'a>(target: &Tref, module: &'a Module) -> Option<(&'a str, &'a str)> {
    let Tref::Ref { id, .. } = target else {
        return None;
    };
    let (lib_name, ty) = id.split_once('#')?;
    let lib = module.ext_libs.iter().find(|l| l.name == lib_name)?;
    let decl = lib.types.iter().find(|t| t.name == ty)?;
    Some((lib.name.as_str(), decl.name.as_str()))
}

/// The handle-method call `call` reaches, resolved against the entry's own
/// fields: the receiver names a foreign-handle field, the method one of its
/// declared methods. `None` when the receiver is not such a field (the
/// frontend/validator already rejected that; nothing to stub).
fn reachable_method<'a>(
    module: &'a Module,
    fields: &'a [EntryField],
    call: &'a OpImplCall,
) -> Option<ReachableExtern<'a>> {
    let head = call.recv.first()?;
    let field = fields.iter().find(|f| f.name == *head)?;
    let (lib, ty) = foreign_handle_type(&field.target, module)?;
    Some(ReachableExtern::Method {
        lib,
        ty,
        method: &call.method,
    })
}

/// Every `extern` call reachable while constructing the entry (a free-fn
/// call sourced on one of its fields, or a handle-method call sourced on
/// one) and, when the test calls an operation, while running that
/// operation's own `impl` body (a handle-method call against a
/// foreign-handle field).
fn reachable_externs<'a>(
    module: &'a Module,
    entry: &'a Shape,
    op: Option<&'a Shape>,
) -> Vec<(ReachableExtern<'a>, Phase)> {
    let mut out = Vec::new();
    let ShapeKind::Entry { fields, .. } = &entry.kind else {
        return out;
    };
    for field in fields {
        if let Some(call) = &field.call {
            out.push((
                ReachableExtern::Free {
                    lib: &call.ns,
                    fn_: &call.func,
                },
                Phase::Construction,
            ));
        }
        if let Some(call) = &field.handle_call {
            out.extend(reachable_method(module, fields, call).map(|c| (c, Phase::Construction)));
        }
    }
    if let Some(op) = op {
        if let ShapeKind::Operation {
            impl_call: Some(call),
            ..
        } = &op.kind
        {
            out.extend(reachable_method(module, fields, call).map(|c| (c, Phase::Call)));
        }
    }
    out
}

/// When a reachable extern call runs, for the diagnostic.
#[derive(Clone, Copy)]
enum Phase {
    Construction,
    Call,
}

/// Every `extern` call reachable from this test's construction/call path
/// must be covered by exactly one `extern_stubs` entry; an unmatched call is
/// a planning-time hard error naming it, the same way an unstubbed `Http`/
/// `Impl` dependency is rejected by [`validate_stub`] -- this is what moves
/// "no stub, no lib installed, fails with a readable diagnostic" to plan
/// time instead of a generated compile/link failure.
pub(super) fn validate_extern_coverage(
    module: &Module,
    entry: &Shape,
    op: Option<&Shape>,
    extern_stubs: &[&ExternStub],
) -> Result<(), String> {
    for (call, phase) in reachable_externs(module, entry, op) {
        let matches = extern_stubs
            .iter()
            .filter(|s| call.matches(&s.target))
            .count();
        let phase = match phase {
            Phase::Construction => "during construction",
            Phase::Call => "during the call",
        };
        if matches == 0 {
            return Err(format!("reaches '{call}' {phase}, which is not stubbed"));
        }
        if matches > 1 {
            return Err(format!("stubs '{call}' more than once"));
        }
    }
    Ok(())
}

/// TypeScript has no adapter behind a handle's generated interface: the
/// interface speaks the foreign shape, and a generated test's fake handle
/// answers in that shape, rebuilt by inverting the method's `returns:`
/// projection over field paths (`vector_extern::raw_answer`). A projection
/// through a `match` has no inverse, so a handle-method stub of such a
/// method cannot be honored in TypeScript; refused here, naming the method,
/// rather than generating a test that answers the wrong shape.
pub(super) fn validate_method_stubs_fakeable_in_typescript(module: &Module) -> Result<(), String> {
    for test in &module.tests {
        for stub in &test.extern_stubs {
            let ExternStubTarget::Method { lib, ty, method } = &stub.target else {
                continue;
            };
            if !stub
                .answers
                .iter()
                .any(|a| matches!(a, StubAnswer::Value { .. }))
            {
                continue;
            }
            let projects_through_match = module
                .ext_libs
                .iter()
                .filter(|l| l.name == *lib)
                .flat_map(|l| l.types.iter().filter(|t| t.name == *ty))
                .flat_map(|t| t.methods.iter().filter(|m| m.name == *method))
                .flat_map(|m| m.langs.iter())
                .filter(|l| l.lang == "ts" || l.lang == "typescript")
                .filter_map(|l| l.returns.as_ref())
                .any(|returns| {
                    returns
                        .fields
                        .iter()
                        .any(|f| matches!(f.value, crate::ir::ReturnsValue::Select(_)))
                });
            if projects_through_match {
                return Err(format!(
                    "test '{}' in module '{}': stubs '{lib}.{ty}.{method}' with a value, but the method's ts binding projects its result through a match, which a TypeScript fake handle cannot answer in the foreign shape; stub the operation or the field that reads it instead",
                    test.name, module.name
                ));
            }
        }
    }
    Ok(())
}

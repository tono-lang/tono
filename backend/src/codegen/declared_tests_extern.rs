//! Extern-call reachability and coverage: which `extern` calls a test's
//! construction/call path can reach (a free-fn call sourced on an entry
//! field, or an opaque handle's method reached through an op's own `impl`
//! body), and validating a declared `extern_stubs` entry against them.
//! Split out of `declared_tests` to keep it under the file-size ceiling;
//! `use super::*` reaches the parent's IR imports and helpers.

use super::*;

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

/// Every `extern` call reachable while constructing the entry (a free-fn
/// call sourced on one of its fields) and, when the test calls an operation,
/// while running that operation's own `impl` body (a handle-method call
/// against a foreign-handle field).
fn reachable_externs<'a>(
    module: &'a Module,
    entry: &'a Shape,
    op: Option<&'a Shape>,
) -> Vec<ReachableExtern<'a>> {
    let mut out = Vec::new();
    let ShapeKind::Entry { fields, .. } = &entry.kind else {
        return out;
    };
    for field in fields {
        if let Some(call) = &field.call {
            out.push(ReachableExtern::Free {
                lib: &call.ns,
                fn_: &call.func,
            });
        }
    }
    if let Some(op) = op {
        if let ShapeKind::Operation {
            impl_call: Some(call),
            ..
        } = &op.kind
        {
            if let Some(head) = call.recv.first() {
                if let Some(field) = fields.iter().find(|f| f.name == *head) {
                    if let Some((lib, ty)) = foreign_handle_type(&field.target, module) {
                        out.push(ReachableExtern::Method {
                            lib,
                            ty,
                            method: &call.method,
                        });
                    }
                }
            }
        }
    }
    out
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
    for call in reachable_externs(module, entry, op) {
        let matches = extern_stubs
            .iter()
            .filter(|s| call.matches(&s.target))
            .count();
        let phase = match call {
            ReachableExtern::Free { .. } => "during construction",
            ReachableExtern::Method { .. } => "during the call",
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

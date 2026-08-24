//! The language-neutral operation model: the async/sync effect and the
//! declared-error discrimination data every target consumes.
//!
//! Both are derived here exactly once so the same operation classifies
//! identically across every generated SDK: a target never re-reads the traits
//! itself. The effect follows the trait when written and is otherwise inferred
//! from the presence of a transport binding; a declared error carries the
//! discrimination key (HTTP status plus an optional body code) and its
//! retryability read off the referenced error shape.

use crate::codegen::casing::{transform, CasingConfig};
use crate::codegen::conventions::{doc_of, rename_of, type_ident_from_id};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::tree::{ClientDecl, Decl, Field, Method, TypeExpr};
use crate::ir::{Member, Model, Module, Shape, ShapeKind, Trait, Tref};

/// Whether an operation performs I/O and therefore waits. How the wait lowers
/// (suspension vs blocking) is a per-language concern; the classification is
/// shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Async,
    Sync,
}

/// Find a trait by its bare name, accepting the `core#`-prefixed spelling too.
/// The rule lives in one place ([`crate::codegen::conventions::core_trait`]) so
/// no reader can drift into matching only the namespaced form, which is not what
/// a real project's frontend emits.
fn find_trait<'a>(traits: &'a [Trait], name: &str) -> Option<&'a Trait> {
    crate::codegen::conventions::core_trait(traits, name)
}

fn has_trait(traits: &[Trait], name: &str) -> bool {
    find_trait(traits, name).is_some()
}

/// Read a trait's single integer argument. The frontend encodes one positional
/// argument as a one-element array; a bare integer value is accepted for
/// hand-authored input.
fn int_arg(t: &Trait) -> Option<i64> {
    match &t.value {
        v if v.is_i64() => v.as_i64(),
        serde_json::Value::Array(items) => items.first().and_then(|v| v.as_i64()),
        _ => None,
    }
}

/// A declared error's body discriminator: the dotted path to probe in the
/// response body (split into segments; `"error.type"` becomes
/// `["error", "type"]`) and the value that path must equal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorCode {
    pub path: Vec<String>,
    pub value: String,
}

/// Read `@errorCode`'s two positional arguments (path, value). The frontend
/// always emits exactly two; a malformed hand-authored array (wrong arity, or
/// an empty path) resolves to no code, the same way a missing trait does.
fn path_and_value_arg(t: &Trait) -> Option<ErrorCode> {
    let items = t.value.as_array()?;
    let [path, value] = items.as_slice() else {
        return None;
    };
    let path = path.as_str()?;
    let value = value.as_str()?;
    if path.is_empty() {
        return None;
    }
    Some(ErrorCode {
        path: path.split('.').map(str::to_string).collect(),
        value: value.to_string(),
    })
}

/// The effect of an operation: an explicit `@async` trait is authoritative;
/// otherwise an operation with a transport binding (`@http`) waits on I/O and
/// is async, and a purely local operation is sync.
pub fn effect_of(op: &Shape) -> Effect {
    if has_trait(&op.traits, "async") || has_trait(&op.traits, "http") {
        Effect::Async
    } else {
        Effect::Sync
    }
}

/// One declared operation error, resolved against its shape: the referenced
/// shape id, the discrimination key (HTTP status, optional body code), and
/// whether the error is retryable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredError {
    /// The referenced error shape's id (as written in the IR).
    pub shape_id: String,
    /// The HTTP status from `@status(n)`. `None` only for input the frontend
    /// would have rejected; such an error still becomes a type but never
    /// enters the discrimination map.
    pub status: Option<i64>,
    /// The body discriminator from `@errorCode(path, value)`, matched against
    /// the response body at that path when several errors share a status.
    pub code: Option<ErrorCode>,
    /// Whether the error carries `@retryable`.
    pub retryable: bool,
}

fn declared_error(shape: &Shape) -> DeclaredError {
    DeclaredError {
        shape_id: shape.id.clone(),
        status: find_trait(&shape.traits, "status").and_then(int_arg),
        code: find_trait(&shape.traits, "errorCode").and_then(path_and_value_arg),
        retryable: has_trait(&shape.traits, "retryable"),
    }
}

fn error_shape_ids(op: &Shape) -> Vec<&str> {
    match &op.kind {
        crate::ir::ShapeKind::Operation { errors, .. } => errors
            .iter()
            .filter_map(|t| match t {
                Tref::Ref { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn shape_by_id<'a>(module: &'a Module, id: &str) -> Option<&'a Shape> {
    module.shapes.iter().find(|s| s.id == id)
}

/// The declared errors of one operation, in declaration order, resolved
/// against the module's shapes. A reference that resolves to no shape is
/// skipped (the frontend reports it); repeats collapse.
pub fn declared_errors(op: &Shape, module: &Module) -> Vec<DeclaredError> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for id in error_shape_ids(op) {
        if seen.contains(&id) {
            continue;
        }
        seen.push(id);
        if let Some(shape) = shape_by_id(module, id) {
            out.push(declared_error(shape));
        }
    }
    out
}

/// Every error shape declared by any of the module's operations (loose ones
/// and the ones nested in an entry body), in order of first appearance. This
/// is the set that becomes error types (under the Api category) in the
/// generated SDK.
pub fn module_declared_errors(module: &Module) -> Vec<DeclaredError> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    let nested = module.shapes.iter().flat_map(|s| match &s.kind {
        ShapeKind::Entry { operations, .. } => operations.as_slice(),
        _ => &[],
    });
    for op in module.operations.iter().chain(nested) {
        for err in declared_errors(op, module) {
            if !seen.contains(&err.shape_id) {
                seen.push(err.shape_id.clone());
                out.push(err);
            }
        }
    }
    out
}

/// Every error shape an `ext` library's own operations declare (a free
/// extern or a handle method), in order of first appearance. These never
/// enter the wire taxonomy (nothing decodes them off a response), but a
/// target that builds one when it recognizes the foreign failure still
/// needs it to be an error value there, even when no module operation
/// declares it.
pub fn ext_declared_errors(module: &Module) -> Vec<DeclaredError> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    let ops = module.ext_libs.iter().flat_map(|lib| {
        lib.externs
            .iter()
            .chain(lib.types.iter().flat_map(|t| t.methods.iter()))
    });
    for id in ops.flat_map(|decl| decl.errors.iter()) {
        if seen.contains(&id.as_str()) {
            continue;
        }
        if let Some(shape) = shape_by_id(module, id) {
            seen.push(id);
            out.push(declared_error(shape));
        }
    }
    out
}

/// The shapes that are only ever a declared operation error and never a
/// member type, an operation input, or an operation output anywhere in the
/// module. A client only ever *decodes* an error response, never encodes one
/// to send, so an `encode` function for such a shape is generated but never
/// called: this is the set a target skips it for. A shape whose whole wire
/// object a declared test pins is kept out of the set: the generated test
/// re-encodes the decoded error for its one total comparison.
pub fn error_only_shapes(module: &Module) -> std::collections::BTreeSet<String> {
    let declared: std::collections::BTreeSet<String> = module_declared_errors(module)
        .into_iter()
        .map(|e| e.shape_id)
        .collect();
    if declared.is_empty() {
        return declared;
    }
    let referenced = referenced_as_data(module);
    let pinned = crate::codegen::declared_tests::error_shapes_pinned_whole(module);
    declared
        .into_iter()
        .filter(|id| {
            !referenced.contains(id) && !pinned.contains(id.rsplit('#').next().unwrap_or(id))
        })
        .collect()
}

/// Every shape id a value would need to be *built* for: a struct/union
/// member's type, an operation's input or output (loose or nested in an
/// entry), an entry field's own type, or a config shape's field type. Doesn't
/// include an operation's declared errors, since those are only ever received,
/// never sent.
fn referenced_as_data(module: &Module) -> std::collections::BTreeSet<String> {
    let mut ids = std::collections::BTreeSet::new();
    let walk = |t: &Tref, ids: &mut std::collections::BTreeSet<String>| {
        fn go(t: &Tref, ids: &mut std::collections::BTreeSet<String>) {
            match t {
                Tref::Ref { id, args } => {
                    ids.insert(id.clone());
                    for arg in args {
                        go(arg, ids);
                    }
                }
                Tref::List(inner) => go(inner, ids),
                Tref::Map(key, value) => {
                    go(key, ids);
                    go(value, ids);
                }
                Tref::Prim(_) | Tref::Param(_) => {}
            }
        }
        go(t, ids);
    };
    let nested_ops = module.shapes.iter().flat_map(|s| match &s.kind {
        ShapeKind::Entry { operations, .. } => operations.as_slice(),
        _ => &[],
    });
    for op in module.operations.iter().chain(nested_ops) {
        if let ShapeKind::Operation { input, output, .. } = &op.kind {
            if let Some(t) = input {
                walk(t, &mut ids);
            }
            if let Some(t) = output {
                walk(t, &mut ids);
            }
        }
    }
    for shape in &module.shapes {
        match &shape.kind {
            ShapeKind::Structure { members, .. } | ShapeKind::Union { members, .. } => {
                for m in members {
                    walk(&m.target, &mut ids);
                }
            }
            ShapeKind::Entry { fields, .. } | ShapeKind::Config { fields } => {
                for f in fields {
                    walk(&f.target, &mut ids);
                }
            }
            _ => {}
        }
    }
    ids
}

/// The declared errors of an operation ordered for discrimination: within one
/// status, code-bearing entries are tried before the codeless catch-all, so a
/// body code is always consulted when it can decide. Declaration order is kept
/// otherwise.
pub fn discrimination_order(op: &Shape, module: &Module) -> Vec<DeclaredError> {
    let mut errors: Vec<DeclaredError> = declared_errors(op, module)
        .into_iter()
        .filter(|e| e.status.is_some())
        .collect();
    // A stable sort keyed only on "has a code" keeps declaration order within
    // each group while moving codeless entries after their status's coded ones.
    errors.sort_by_key(|e| e.code.is_none());
    errors
}

/// The declared-error type name: the shape's PascalCase local name, matching
/// how every target names the shape's own declaration.
pub fn error_type_name(err: &DeclaredError) -> String {
    type_ident_from_id(&err.shape_id)
}

/// The canonical error-surface type names, derived through the same casing
/// engine as every other type identifier (so `api_error` follows the
/// initialism set). Each target consumes the subset its idiom needs: Go has no
/// root, Rust alone has the Api payload enum.
pub struct ErrorNames {
    pub root: String,
    pub api: String,
    pub api_failure: String,
    pub validation: String,
    pub transport: String,
    pub decode: String,
    pub contract: String,
    pub config: String,
    pub violation: String,
}

pub fn error_names() -> ErrorNames {
    ErrorNames {
        root: type_ident_from_id("tono_error"),
        api: type_ident_from_id("api_error"),
        api_failure: type_ident_from_id("api_failure"),
        validation: type_ident_from_id("validation_error"),
        transport: type_ident_from_id("transport_error"),
        decode: type_ident_from_id("decode_error"),
        contract: type_ident_from_id("contract_error"),
        config: type_ident_from_id("config_error"),
        violation: type_ident_from_id("violation"),
    }
}

/// The generated method identifier for an operation, honoring `@rename(lang)`.
pub fn method_ident(op: &Shape, config: &CasingConfig, lang: &str) -> String {
    let local = op.id.rsplit('#').next().unwrap_or(&op.id);
    let rename = rename_of(&op.traits, lang);
    transform(local, SymbolKind::Method, config, rename.as_deref())
}

/// An operation's input and output type references.
pub fn op_io(op: &Shape) -> (Option<&Tref>, Option<&Tref>) {
    match &op.kind {
        ShapeKind::Operation { input, output, .. } => (input.as_ref(), output.as_ref()),
        _ => (None, None),
    }
}

/// Whether the operation declared a nullable (`T?`) return: the call can
/// yield no value, and every target spells the return type accordingly.
pub fn op_output_nullable(op: &Shape) -> bool {
    match &op.kind {
        ShapeKind::Operation {
            output_nullable, ..
        } => *output_nullable,
        _ => false,
    }
}

/// The typed wire binding a Protocol pass resolved for this operation, meant
/// to be read directly by a target. `None` for a purely local operation.
pub fn wire_binding(op: &Shape) -> Option<&crate::ir::WireBinding> {
    match &op.kind {
        ShapeKind::Operation { wire, .. } => wire.as_deref(),
        _ => None,
    }
}

/// An op's own `impl .field.method(args)` body, when it has one.
/// `None` for a purely local operation, a wire-bound one, or one implemented
/// through the legacy `ext impl` extension.
pub fn op_impl_call(op: &Shape) -> Option<&crate::ir::OpImplCall> {
    match &op.kind {
        ShapeKind::Operation { impl_call, .. } => impl_call.as_ref(),
        _ => None,
    }
}

/// The op's declared parameter name (`op fetch(ref: note_ref): note` names
/// `ref`), the name a `.ref...` reference inside the op body resolves
/// against. Absent for the legacy unnamed input form.
pub fn input_name(op: &Shape) -> Option<&str> {
    match &op.kind {
        ShapeKind::Operation { input_name, .. } => input_name.as_deref(),
        _ => None,
    }
}

/// The member a `WireValue::Param`/`TemplatePart::Param` segment names: the
/// op's declared parameter type, resolved to a same-module structure, then
/// the one member matching `seg`. `None` when the parameter type is not a
/// same-module structure (a cross-module reference the target does not chase
/// here) or has no such member; the caller falls back to the decoded record
/// in that case. The typechecker only ever resolves a param reference one
/// segment deep, so `seg` is always the whole path.
pub fn param_member<'a>(module: &'a Module, input: Option<&Tref>, seg: &str) -> Option<&'a Member> {
    let Tref::Ref { id, .. } = input? else {
        return None;
    };
    let shape = shape_by_id(module, id)?;
    match &shape.kind {
        ShapeKind::Structure { members, .. } => members.iter().find(|m| m.name == seg),
        _ => None,
    }
}

/// Build the client declaration: one method signature per operation, the
/// effect classified here and lowered by the target's render rules. The
/// per-language pieces ride the parameters, exactly like `emit_shape`: the
/// target's `type_expr_of` resolves the input/output types, and `err` names
/// its error-channel type (`None` where errors are thrown).
pub fn client_decl(
    module: &Module,
    config: &CasingConfig,
    lang: &str,
    type_expr_of: &impl Fn(&Tref) -> TypeExpr,
    err: Option<&str>,
) -> Decl {
    let err = err.map(|name| TypeExpr::Ref(Symbol::builtin(name)));
    let methods = module
        .operations
        .iter()
        .map(|op| {
            let (input, output) = op_io(op);
            Method {
                name: Symbol::builtin(method_ident(op, config, lang)),
                params: input
                    .map(|t| {
                        vec![Field {
                            name: Symbol::builtin("input"),
                            ty: type_expr_of(t),
                            nullable: false,
                            wire: None,
                            deprecated: None,
                            doc: None,
                        }]
                    })
                    .unwrap_or_default(),
                ret: output.map(|t| {
                    let ty = type_expr_of(t);
                    if op_output_nullable(op) {
                        TypeExpr::nullable(ty)
                    } else {
                        ty
                    }
                }),
                err: err.clone(),
                is_async: effect_of(op) == Effect::Async,
                doc: doc_of(&op.traits),
            }
        })
        .collect();
    Decl::Client(ClientDecl {
        name: Symbol::builtin(type_ident_from_id("client")),
        methods,
    })
}

/// Reject an operation whose declared errors cannot be told apart: two errors
/// sharing a status and the same coded discriminator (same path, same value)
/// collide silently at runtime, since the generated guard for one is
/// identical to the other's. A codeless error is that status's catch-all by
/// rule, not by declaration order, so it never collides with a coded sibling.
/// Returns the first offense, naming both shapes.
pub fn validate_error_codes(model: &Model) -> Result<(), String> {
    for module in &model.modules {
        let nested = module.shapes.iter().flat_map(|s| match &s.kind {
            ShapeKind::Entry { operations, .. } => operations.as_slice(),
            _ => &[],
        });
        for op in module.operations.iter().chain(nested) {
            let errors = declared_errors(op, module);
            let mut seen: Vec<(i64, &[String], &str, &str)> = Vec::new();
            for err in &errors {
                let (Some(status), Some(code)) = (err.status, &err.code) else {
                    continue;
                };
                if let Some((_, _, _, first_id)) = seen.iter().find(|(s, p, v, _)| {
                    *s == status && *p == code.path.as_slice() && *v == code.value
                }) {
                    return Err(format!(
                        "module {}: operation {} cannot discriminate between errors {} and {}: \
                         both use status {status} with @errorCode(\"{}\", \"{}\")",
                        module.name,
                        op.id.rsplit('#').next().unwrap_or(&op.id),
                        first_id,
                        err.shape_id,
                        code.path.join("."),
                        code.value,
                    ));
                }
                seen.push((
                    status,
                    code.path.as_slice(),
                    code.value.as_str(),
                    &err.shape_id,
                ));
            }
        }
    }
    Ok(())
}

/// Build one discrimination declaration per operation that declares errors,
/// handing the target's builder the errors already in discrimination order
/// (coded entries before a codeless catch-all on the same status).
pub fn discriminator_decls(
    module: &Module,
    build: impl Fn(&Shape, &[DeclaredError]) -> Decl,
) -> Vec<Decl> {
    module
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .map(|op| build(op, &discrimination_order(op, module)))
        .collect()
}

#[cfg(test)]
#[path = "ops_tests.rs"]
mod tests;

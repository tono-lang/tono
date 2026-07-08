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
use crate::codegen::conventions::{rename_of, type_ident_from_id};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::tree::{ClientDecl, Decl, Field, Method, TypeExpr};
use crate::ir::{Module, Shape, ShapeKind, Trait, Tref};

/// Whether an operation performs I/O and therefore waits. How the wait lowers
/// (suspension vs blocking) is a per-language concern; the classification is
/// shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    Async,
    Sync,
}

/// Find a trait by its bare name, accepting the `core#`-prefixed spelling too:
/// the frontend emits bare ids today while hand-authored fixtures already use
/// the namespaced form the future name-resolution pass will produce.
fn find_trait<'a>(traits: &'a [Trait], name: &str) -> Option<&'a Trait> {
    traits
        .iter()
        .find(|t| t.id == name || t.id.strip_prefix("core#") == Some(name))
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

/// Read a trait's single string argument, with the same tolerance as
/// [`int_arg`].
fn string_arg(t: &Trait) -> Option<String> {
    match &t.value {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Array(items) => {
            items.first().and_then(|v| v.as_str()).map(str::to_string)
        }
        _ => None,
    }
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
    /// The body discriminator value from `@errorCode("...")`, matched against
    /// the response body's `code` field when several errors share a status.
    pub code: Option<String>,
    /// Whether the error carries `@retryable`.
    pub retryable: bool,
}

fn declared_error(shape: &Shape) -> DeclaredError {
    DeclaredError {
        shape_id: shape.id.clone(),
        status: find_trait(&shape.traits, "status").and_then(int_arg),
        code: find_trait(&shape.traits, "errorCode").and_then(string_arg),
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

/// Every error shape declared by any of the module's operations, in order of
/// first appearance. This is the set that becomes error types (under the Api
/// category) in the generated SDK.
pub fn module_declared_errors(module: &Module) -> Vec<DeclaredError> {
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for op in &module.operations {
        for err in declared_errors(op, module) {
            if !seen.contains(&err.shape_id) {
                seen.push(err.shape_id.clone());
                out.push(err);
            }
        }
    }
    out
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
        violation: type_ident_from_id("violation"),
    }
}

/// The generated method identifier for an operation, honoring `@rename(lang)`.
fn method_ident(op: &Shape, config: &CasingConfig, lang: &str) -> String {
    let local = op.id.rsplit('#').next().unwrap_or(&op.id);
    let rename = rename_of(&op.traits, lang);
    transform(local, SymbolKind::Method, config, rename.as_deref())
}

fn op_io(op: &Shape) -> (Option<&Tref>, Option<&Tref>) {
    match &op.kind {
        ShapeKind::Operation { input, output, .. } => (input.as_ref(), output.as_ref()),
        _ => (None, None),
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
                        }]
                    })
                    .unwrap_or_default(),
                ret: output.map(type_expr_of),
                err: err.clone(),
                is_async: effect_of(op) == Effect::Async,
            }
        })
        .collect();
    Decl::Client(ClientDecl {
        name: Symbol::builtin(type_ident_from_id("client")),
        methods,
    })
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
mod tests {
    use super::*;
    use crate::ir::{Shape, ShapeKind};
    use serde_json::json;

    fn trait_of(id: &str, value: serde_json::Value) -> Trait {
        Trait {
            id: id.into(),
            value,
        }
    }

    fn op(traits: Vec<Trait>, errors: Vec<&str>) -> Shape {
        Shape {
            id: "m#do_thing".into(),
            kind: ShapeKind::Operation {
                input: None,
                output: None,
                errors: errors
                    .into_iter()
                    .map(|id| Tref::Ref {
                        id: id.into(),
                        args: vec![],
                    })
                    .collect(),
            },
            traits,
        }
    }

    fn error_shape(id: &str, traits: Vec<Trait>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits,
        }
    }

    fn module(shapes: Vec<Shape>, operations: Vec<Shape>) -> Module {
        Module {
            name: "m".into(),
            shapes,
            operations,
        }
    }

    #[test]
    fn the_async_trait_is_authoritative_and_transport_infers_async() {
        // Explicit @async, with or without a transport, is async.
        assert_eq!(
            effect_of(&op(vec![trait_of("async", json!(null))], vec![])),
            Effect::Async
        );
        // A transport binding alone infers async.
        assert_eq!(
            effect_of(&op(
                vec![trait_of("http", json!({"method": "POST"}))],
                vec![]
            )),
            Effect::Async
        );
        // A purely local operation is sync.
        assert_eq!(effect_of(&op(vec![], vec![])), Effect::Sync);
        // The namespaced spelling counts too.
        assert_eq!(
            effect_of(&op(vec![trait_of("core#async", json!(null))], vec![])),
            Effect::Async
        );
    }

    #[test]
    fn declared_errors_resolve_status_code_and_retryable() {
        let module = module(
            vec![
                error_shape(
                    "m#payment_declined",
                    vec![
                        trait_of("status", json!([402])),
                        trait_of("errorCode", json!(["payment_declined"])),
                        trait_of("retryable", json!(null)),
                    ],
                ),
                error_shape("m#rate_limited", vec![trait_of("status", json!(429))]),
            ],
            vec![],
        );
        let op = op(vec![], vec!["m#payment_declined", "m#rate_limited"]);
        let errors = declared_errors(&op, &module);
        assert_eq!(
            errors,
            vec![
                DeclaredError {
                    shape_id: "m#payment_declined".into(),
                    status: Some(402),
                    code: Some("payment_declined".into()),
                    retryable: true,
                },
                DeclaredError {
                    shape_id: "m#rate_limited".into(),
                    status: Some(429),
                    code: None,
                    retryable: false,
                },
            ]
        );
    }

    #[test]
    fn unresolved_and_repeated_references_are_skipped() {
        let module = module(
            vec![error_shape(
                "m#not_found",
                vec![trait_of("status", json!([404]))],
            )],
            vec![],
        );
        let op = op(vec![], vec!["m#not_found", "m#nope", "m#not_found"]);
        let errors = declared_errors(&op, &module);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].shape_id, "m#not_found");
    }

    #[test]
    fn module_errors_are_the_union_in_first_appearance_order() {
        let shapes = vec![
            error_shape("m#a", vec![trait_of("status", json!([400]))]),
            error_shape("m#b", vec![trait_of("status", json!([404]))]),
        ];
        let op_one = op(vec![], vec!["m#b", "m#a"]);
        let op_two = op(vec![], vec!["m#a"]);
        let module = module(shapes, vec![op_one, op_two]);
        let ids: Vec<String> = module_declared_errors(&module)
            .into_iter()
            .map(|e| e.shape_id)
            .collect();
        assert_eq!(ids, vec!["m#b".to_string(), "m#a".to_string()]);
    }

    #[test]
    fn the_discriminator_driver_skips_operations_with_no_declared_errors() {
        let shapes = vec![error_shape("m#nf", vec![trait_of("status", json!([404]))])];
        let with_errors = op(vec![], vec!["m#nf"]);
        let without = op(vec![], vec![]);
        let module = module(shapes, vec![without, with_errors]);
        let decls = discriminator_decls(&module, |_, ordered| {
            crate::codegen::tree::Decl::raw(format!("{} entries", ordered.len()))
        });
        // Only the error-declaring operation gets a declaration.
        assert_eq!(decls.len(), 1);
    }

    #[test]
    fn discrimination_tries_coded_entries_before_the_codeless_catch_all() {
        let shapes = vec![
            error_shape("m#generic_bad", vec![trait_of("status", json!([400]))]),
            error_shape(
                "m#coded_bad",
                vec![
                    trait_of("status", json!([400])),
                    trait_of("errorCode", json!(["specific"])),
                ],
            ),
            // A shape the frontend would have rejected: no status. It never
            // enters the discrimination map.
            error_shape("m#no_status", vec![]),
        ];
        let op = op(vec![], vec!["m#generic_bad", "m#coded_bad", "m#no_status"]);
        let module = module(shapes, vec![]);
        let ordered: Vec<String> = discrimination_order(&op, &module)
            .into_iter()
            .map(|e| e.shape_id)
            .collect();
        assert_eq!(
            ordered,
            vec!["m#coded_bad".to_string(), "m#generic_bad".to_string()]
        );
    }
}

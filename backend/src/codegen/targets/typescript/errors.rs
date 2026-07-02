//! The TypeScript error surface and client interface: the closed taxonomy
//! rooted at `TonoError`, one class per declared operation error under the
//! `Api` category, the per-operation discrimination function, and the client
//! interface whose async operations return `Promise`s.
//!
//! TypeScript discriminates with `instanceof`, so the taxonomy is a class
//! hierarchy: every category (and every declared error) descends from the
//! canonical root `TonoError`. A declared error extends the fallback
//! `ApiError`-shaped class because both live inside the `Api` category, which
//! is exactly what lets a caller catch the whole category with one
//! `instanceof` while still receiving the concrete fallback type when a
//! response matches no declared error.

use crate::codegen::casing::{transform, CasingConfig};
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::ops::{
    declared_errors, discrimination_order, effect_of, module_declared_errors, DeclaredError,
    Effect,
};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::tree::{ClientDecl, Decl, Field, FnBody, Function, Method, Raw, TypeExpr};
use crate::ir::{Module, Shape};

/// The canonical taxonomy type names, derived through the same casing engine
/// as every other type identifier (so `api_error` follows the initialism set).
struct Names {
    root: String,
    api: String,
    validation: String,
    transport: String,
    decode: String,
    contract: String,
    violation: String,
}

fn names() -> Names {
    Names {
        root: type_ident_from_id("tono_error"),
        api: type_ident_from_id("api_error"),
        validation: type_ident_from_id("validation_error"),
        transport: type_ident_from_id("transport_error"),
        decode: type_ident_from_id("decode_error"),
        contract: type_ident_from_id("contract_error"),
        violation: type_ident_from_id("violation"),
    }
}

/// The class name of a declared error: its type name plus an `Error` suffix,
/// so the class never collides with the shape's own data interface in the
/// types file.
fn declared_class_name(err: &DeclaredError) -> String {
    format!("{}Error", type_ident_from_id(&err.shape_id))
}

fn raw(text: String) -> Decl {
    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
    })
}

/// The closed error taxonomy: the abstract root, the `Violation` record, and
/// the five category classes. Emitted once per module that has operations.
pub fn taxonomy_decls() -> Vec<Decl> {
    let n = names();
    let root = &n.root;
    vec![
        raw(format!(
            "export abstract class {root} extends Error {{\n  retryable(): boolean {{\n    return false;\n  }}\n}}"
        )),
        raw(format!(
            "export interface {} {{\n  field: string;\n  constraint: string;\n  message: string;\n}}",
            n.violation
        )),
        raw(format!(
            "export class {} extends {root} {{\n  constructor(readonly violations: {}[]) {{\n    super(\"validation failed\");\n    this.name = \"{}\";\n  }}\n}}",
            n.validation, n.violation, n.validation
        )),
        raw(format!(
            "export class {} extends {root} {{\n  constructor(readonly cause: unknown) {{\n    super(\"transport failure\");\n    this.name = \"{}\";\n  }}\n}}",
            n.transport, n.transport
        )),
        raw(format!(
            "export class {} extends {root} {{\n  constructor(readonly path: string, readonly expected: string, readonly raw: string) {{\n    super(\"response body did not match the declared schema\");\n    this.name = \"{}\";\n  }}\n}}",
            n.decode, n.decode
        )),
        raw(format!(
            "export class {} extends {root} {{\n  constructor(readonly contractName: string, readonly cause: unknown) {{\n    super(\"contract hook failed\");\n    this.name = \"{}\";\n  }}\n}}",
            n.contract, n.contract
        )),
        raw(format!(
            "export class {} extends {root} {{\n  constructor(readonly status: number, readonly body: string) {{\n    super(`api error ${{status}}`);\n    this.name = \"{}\";\n  }}\n}}",
            n.api, n.api
        )),
    ]
}

/// One class per declared operation error, under the `Api` category. The
/// decoded body rides a `data` field (never spread into the class) so a shape
/// field can never collide with the inherited `status`/`body`/`message`.
pub fn declared_error_decls(module: &Module) -> Vec<Decl> {
    let n = names();
    module_declared_errors(module)
        .iter()
        .map(|err| {
            let class = declared_class_name(err);
            let data = type_ident_from_id(&err.shape_id);
            let status = err.status.unwrap_or(0);
            let retryable = if err.retryable {
                format!("\n  retryable(): boolean {{\n    return true;\n  }}")
            } else {
                String::new()
            };
            raw(format!(
                "export class {class} extends {} {{\n  constructor(readonly data: {data}, body: string) {{\n    super({status}, body);\n    this.name = \"{class}\";\n  }}{retryable}\n}}",
                n.api
            ))
        })
        .collect()
}

/// The generated method identifier for an operation.
fn method_ident(op: &Shape, config: &CasingConfig) -> String {
    let local = op.id.rsplit('#').next().unwrap_or(&op.id);
    let rename = crate::codegen::conventions::rename_of(&op.traits, LANG);
    transform(local, SymbolKind::Method, config, rename.as_deref())
}

fn op_io(op: &Shape) -> (Option<&crate::ir::Tref>, Option<&crate::ir::Tref>) {
    match &op.kind {
        crate::ir::ShapeKind::Operation { input, output, .. } => {
            (input.as_ref(), output.as_ref())
        }
        _ => (None, None),
    }
}

/// The client interface: one method per operation. An async operation returns
/// a `Promise`; errors are thrown, so the error channel stays out of the
/// signature.
pub fn client_decl(module: &Module, config: &CasingConfig) -> Decl {
    let methods = module
        .operations
        .iter()
        .map(|op| {
            let (input, output) = op_io(op);
            Method {
                name: Symbol::builtin(method_ident(op, config)),
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
                err: None,
                is_async: effect_of(op) == Effect::Async,
            }
        })
        .collect();
    Decl::Client(ClientDecl {
        name: Symbol::builtin(type_ident_from_id("client")),
        methods,
    })
}

/// A self-module type symbol, imported from the types file via the serde
/// file's companion redirect.
fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// The per-operation discrimination functions, one per operation that declares
/// errors: `(status, raw body) -> TonoError`. The mapping tries the declared
/// errors (coded entries before a codeless catch-all on the same status) and
/// resolves everything else to the concrete fallback type, never the whole
/// `Api` category.
pub fn discriminator_decls(module: &Module) -> Vec<Decl> {
    let n = names();
    module
        .operations
        .iter()
        .filter(|op| !declared_errors(op, module).is_empty())
        .map(|op| discriminator_fn(op, module, &n))
        .collect()
}

fn discriminator_fn(op: &Shape, module: &Module, n: &Names) -> Decl {
    let ordered = discrimination_order(op, module);
    let fallback = format!("new {}(status, body)", n.api);
    let mut body = String::new();
    body.push_str("  let parsed: any;\n");
    body.push_str(&format!(
        "  try {{\n    parsed = JSON.parse(body);\n  }} catch {{\n    return {fallback};\n  }}\n"
    ));
    if ordered.iter().any(|e| e.code.is_some()) {
        body.push_str(
            "  const code = typeof parsed === \"object\" && parsed !== null ? parsed[\"code\"] : undefined;\n",
        );
    }
    body.push_str("  try {\n");
    let mut refs: Vec<Symbol> = vec![
        module_symbol(&n.root, module),
        module_symbol(&n.api, module),
    ];
    for err in &ordered {
        let class = declared_class_name(err);
        let data = type_ident_from_id(&err.shape_id);
        let status = err.status.unwrap_or(0);
        let guard = match &err.code {
            Some(code) => format!("status === {status} && code === \"{code}\""),
            None => format!("status === {status}"),
        };
        body.push_str(&format!(
            "    if ({guard}) {{\n      return new {class}(decode{data}(parsed), body);\n    }}\n"
        ));
        refs.push(module_symbol(&class, module));
    }
    // The declared-error decode can itself throw; an undecodable declared
    // match falls back to the generic type so new server fields or shapes
    // never break the caller (forward-compat).
    body.push_str("  } catch {}\n");
    body.push_str(&format!("  return {fallback};"));

    let fn_name = format!("decode{}Error", type_ident_from_id(&op.id));
    Decl::Function(Function {
        name: Symbol::builtin(fn_name),
        params: vec![
            Field {
                name: Symbol::builtin("status"),
                ty: TypeExpr::Ref(Symbol::builtin("number")),
                nullable: false,
                wire: None,
            },
            Field {
                name: Symbol::builtin("body"),
                ty: TypeExpr::Ref(Symbol::builtin("string")),
                nullable: false,
                wire: None,
            },
        ],
        ret: Some(TypeExpr::Ref(module_symbol(&n.root, module))),
        body: FnBody::Raw { text: body, refs },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::{member, structure};
    use crate::ir::{Prim, ShapeKind, Trait, Tref};
    use serde_json::json;

    fn trait_of(id: &str, value: serde_json::Value) -> Trait {
        Trait {
            id: id.into(),
            value,
        }
    }

    fn op(id: &str, traits: Vec<Trait>, errors: Vec<&str>) -> Shape {
        Shape {
            id: id.into(),
            kind: ShapeKind::Operation {
                input: Some(Tref::Ref {
                    id: "m#charge_input".into(),
                    args: vec![],
                }),
                output: Some(Tref::Ref {
                    id: "m#charge".into(),
                    args: vec![],
                }),
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

    fn demo_module() -> Module {
        let mut declined = structure(
            "m#payment_declined",
            vec![member("message", Tref::Prim(Prim::String), true)],
        );
        declined.traits = vec![
            trait_of("status", json!([402])),
            trait_of("errorCode", json!(["payment_declined"])),
            trait_of("retryable", json!(null)),
        ];
        let mut limited = structure(
            "m#rate_limited",
            vec![member("retry_after_seconds", Tref::Prim(Prim::I64), true)],
        );
        limited.traits = vec![trait_of("status", json!([429]))];
        Module {
            name: "m".into(),
            shapes: vec![
                structure("m#charge", vec![member("id", Tref::Prim(Prim::String), true)]),
                structure(
                    "m#charge_input",
                    vec![member("amount", Tref::Prim(Prim::I64), true)],
                ),
                declined,
                limited,
            ],
            operations: vec![op(
                "m#create_charge",
                vec![trait_of("http", json!({"method": "POST", "path": "/charges"}))],
                vec!["m#payment_declined", "m#rate_limited"],
            )],
        }
    }

    fn rendered(decls: &[Decl]) -> String {
        decls
            .iter()
            .map(|d| TsRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_taxonomy_is_five_categories_rooted_at_tono_error() {
        let out = rendered(&taxonomy_decls());
        assert!(out.contains("export abstract class TonoError extends Error {"));
        for category in [
            "ValidationError",
            "TransportError",
            "DecodeError",
            "ContractError",
            "APIError",
        ] {
            assert!(
                out.contains(&format!("export class {category} extends TonoError {{")),
                "{category} must extend the root"
            );
        }
        // The root's default predicate reports non-retryable.
        assert!(out.contains("retryable(): boolean {\n    return false;"));
        // Exactly the five categories: no sixth class extends the root.
        assert_eq!(out.matches("extends TonoError {").count(), 5);
    }

    #[test]
    fn declared_errors_become_classes_under_the_api_category() {
        let out = rendered(&declared_error_decls(&demo_module()));
        assert!(out.contains("export class PaymentDeclinedError extends APIError {"));
        assert!(out.contains("constructor(readonly data: PaymentDeclined, body: string) {"));
        assert!(out.contains("super(402, body);"));
        // @retryable overrides the root predicate; its absence inherits false.
        assert!(out.contains("retryable(): boolean {\n    return true;"));
        assert!(out.contains("export class RateLimitedError extends APIError {"));
        assert!(out.contains("super(429, body);"));
    }

    #[test]
    fn the_client_interface_lowers_the_effect_to_promise() {
        let module = demo_module();
        let out = TsRules.render_decl(&client_decl(&module, &ts_casing()));
        assert_eq!(
            out,
            "export interface Client {\n  createCharge(input: ChargeInput): Promise<Charge>;\n}"
        );
    }

    #[test]
    fn a_sync_operation_keeps_a_plain_signature() {
        let mut module = demo_module();
        module.operations = vec![op("m#local_sum", vec![], vec![])];
        let out = TsRules.render_decl(&client_decl(&module, &ts_casing()));
        assert!(out.contains("localSum(input: ChargeInput): Charge;"));
        assert!(!out.contains("Promise"));
    }

    #[test]
    fn the_discriminator_maps_status_and_code_and_falls_back_to_api_error() {
        let module = demo_module();
        let out = rendered(&discriminator_decls(&module));
        assert!(out.contains(
            "export function decodeCreateChargeError(status: number, body: string): TonoError {"
        ));
        // The coded entry consults the body's code field; the codeless one
        // matches on status alone; anything else is the concrete fallback.
        assert!(out.contains("if (status === 402 && code === \"payment_declined\") {"));
        assert!(out.contains("return new PaymentDeclinedError(decodePaymentDeclined(parsed), body);"));
        assert!(out.contains("if (status === 429) {"));
        assert!(out.contains("return new RateLimitedError(decodeRateLimited(parsed), body);"));
        assert!(out.contains("return new APIError(status, body);"));
    }

    #[test]
    fn coded_entries_are_tried_before_a_codeless_error_on_the_same_status() {
        let mut module = demo_module();
        let mut coded = structure("m#coded_bad", vec![]);
        coded.traits = vec![
            trait_of("status", json!([400])),
            trait_of("errorCode", json!(["specific"])),
        ];
        let mut catch_all = structure("m#generic_bad", vec![]);
        catch_all.traits = vec![trait_of("status", json!([400]))];
        module.shapes.push(coded);
        module.shapes.push(catch_all);
        module.operations = vec![op(
            "m#do_thing",
            vec![],
            vec!["m#generic_bad", "m#coded_bad"],
        )];
        let out = rendered(&discriminator_decls(&module));
        let coded_at = out.find("code === \"specific\"").expect("coded guard");
        let catch_all_at = out.find("if (status === 400) {").expect("catch-all guard");
        assert!(coded_at < catch_all_at, "the coded guard must run first");
    }

    #[test]
    fn an_operation_with_no_declared_errors_gets_no_discriminator() {
        let mut module = demo_module();
        module.operations = vec![op("m#ping", vec![], vec![])];
        assert!(discriminator_decls(&module).is_empty());
    }
}

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

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::ops::{
    self, error_names, error_type_name, module_declared_errors, DeclaredError, ErrorNames,
};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr};
use crate::ir::{Module, Shape};

/// The declarations for the types file: the taxonomy, the declared-error
/// classes, and the client interface.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let mut decls = taxonomy_decls();
    decls.extend(declared_error_decls(module));
    // Errors are thrown in TypeScript, so the client's error channel stays out
    // of the signatures (`None`).
    decls.push(ops::client_decl(module, config, LANG, &type_expr_of, None));
    decls
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| {
        discriminator_fn(op, ordered, module, &n)
    })
}

/// The class name of a declared error: its type name plus an `Error` suffix,
/// so the class never collides with the shape's own data interface in the
/// types file.
fn declared_class_name(err: &DeclaredError) -> String {
    format!("{}Error", error_type_name(err))
}

/// The closed error taxonomy: the abstract root, the `Violation` record, and
/// the five category classes.
fn taxonomy_decls() -> Vec<Decl> {
    let n = error_names();
    let root = &n.root;
    let category = |name: &str, ctor_params: &str, message: &str| {
        Decl::raw(format!(
            "export class {name} extends {root} {{\n  constructor({ctor_params}) {{\n    super({message});\n    this.name = \"{name}\";\n  }}\n}}"
        ))
    };
    vec![
        Decl::raw(format!(
            "export abstract class {root} extends Error {{\n  retryable(): boolean {{\n    return false;\n  }}\n}}"
        )),
        Decl::raw(format!(
            "export interface {} {{\n  field: string;\n  constraint: string;\n  message: string;\n}}",
            n.violation
        )),
        category(
            &n.validation,
            &format!("readonly violations: {}[]", n.violation),
            "\"validation failed\"",
        ),
        category(
            &n.transport,
            "readonly cause: unknown",
            "\"transport failure\"",
        ),
        category(
            &n.decode,
            "readonly path: string, readonly expected: string, readonly raw: string",
            "\"response body did not match the declared schema\"",
        ),
        category(
            &n.contract,
            "readonly contractName: string, readonly cause: unknown",
            "\"contract hook failed\"",
        ),
        category(
            &n.api,
            "readonly status: number, readonly body: string",
            "`api error ${status}`",
        ),
    ]
}

/// One class per declared operation error, under the `Api` category. The
/// decoded body rides a `data` field (never spread into the class) so a shape
/// field can never collide with the inherited `status`/`body`/`message`.
fn declared_error_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    module_declared_errors(module)
        .iter()
        .map(|err| {
            let class = declared_class_name(err);
            let data = error_type_name(err);
            let status = err.status.unwrap_or(0);
            let retryable = if err.retryable {
                "\n  retryable(): boolean {\n    return true;\n  }"
            } else {
                ""
            };
            Decl::raw(format!(
                "export class {class} extends {} {{\n  constructor(readonly data: {data}, body: string) {{\n    super({status}, body);\n    this.name = \"{class}\";\n  }}{retryable}\n}}",
                n.api
            ))
        })
        .collect()
}

/// A self-module type symbol, imported from the types file via the serde
/// file's companion redirect.
fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// One discrimination function: `(status, raw body) -> TonoError`. The mapping
/// tries the declared errors and resolves everything else to the concrete
/// fallback type, never the whole `Api` category.
fn discriminator_fn(
    op: &Shape,
    ordered: &[DeclaredError],
    module: &Module,
    n: &ErrorNames,
) -> Decl {
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
    for err in ordered {
        let class = declared_class_name(err);
        let data = error_type_name(err);
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
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::{error_demo_module, error_shape, operation, rendered};

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &ts_casing()), &TsRules)
    }

    #[test]
    fn the_taxonomy_is_five_categories_rooted_at_tono_error() {
        let out = types_text(&error_demo_module());
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
        let out = types_text(&error_demo_module());
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
        let out = types_text(&error_demo_module());
        assert!(out.contains(
            "export interface Client {\n  createCharge(input: ChargeInput): Promise<Charge>;\n}"
        ));
    }

    #[test]
    fn a_sync_operation_keeps_a_plain_signature() {
        let mut module = error_demo_module();
        module.operations = vec![operation("m#local_sum", vec![], vec![])];
        let out = types_text(&module);
        assert!(out.contains("localSum(input: ChargeInput): Charge;"));
        assert!(!out.contains("Promise"));
    }

    #[test]
    fn the_discriminator_maps_status_and_code_and_falls_back_to_api_error() {
        let out = rendered(&serde_decls(&error_demo_module()), &TsRules);
        assert!(out.contains(
            "export function decodeCreateChargeError(status: number, body: string): TonoError {"
        ));
        // The coded entry consults the body's code field; the codeless one
        // matches on status alone; anything else is the concrete fallback.
        assert!(out.contains("if (status === 402 && code === \"payment_declined\") {"));
        assert!(
            out.contains("return new PaymentDeclinedError(decodePaymentDeclined(parsed), body);")
        );
        assert!(out.contains("if (status === 429) {"));
        assert!(out.contains("return new RateLimitedError(decodeRateLimited(parsed), body);"));
        assert!(out.contains("return new APIError(status, body);"));
    }

    #[test]
    fn coded_entries_are_tried_before_a_codeless_error_on_the_same_status() {
        let mut module = error_demo_module();
        module.shapes.push(error_shape(
            "m#coded_bad",
            vec![],
            400,
            Some("specific"),
            false,
        ));
        module
            .shapes
            .push(error_shape("m#generic_bad", vec![], 400, None, false));
        module.operations = vec![operation(
            "m#do_thing",
            vec![],
            vec!["m#generic_bad", "m#coded_bad"],
        )];
        let out = rendered(&serde_decls(&module), &TsRules);
        let coded_at = out.find("code === \"specific\"").expect("coded guard");
        let catch_all_at = out.find("if (status === 400) {").expect("catch-all guard");
        assert!(coded_at < catch_all_at, "the coded guard must run first");
    }
}

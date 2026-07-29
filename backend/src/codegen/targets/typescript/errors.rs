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
use crate::codegen::ops::{
    self, error_names, error_type_name, module_declared_errors, DeclaredError, ErrorNames,
};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::taxonomy::TaxonomyLiveness;
use crate::codegen::tree::{Decl, Field, FnBody, Function, TypeExpr};
use crate::ir::{Module, Shape};

/// The declarations for the types file: the taxonomy, the declared-error
/// classes, and the client interface.
///
/// Unlike Rust/Go, a loose (non-entry) operation still gets a concrete
/// `HttpClient` in TypeScript, so `liveness` here is real, derived liveness
/// (not [`TaxonomyLiveness::all_live`]): the caller passes what
/// [`crate::codegen::taxonomy::derive`] reports for this module.
pub fn type_decls(
    module: &Module,
    config: &CasingConfig,
    liveness: &TaxonomyLiveness,
) -> Vec<Decl> {
    let mut decls = taxonomy_and_declared_decls(module, liveness);
    // Errors are thrown in TypeScript, so the client's error channel stays out
    // of the signatures (`None`).
    decls.push(ops::client_decl(module, config, LANG, &type_expr_of, None));
    decls
}

/// The taxonomy and the declared-error classes without the loose-op client
/// interface: what an entry-only module needs (its client surface is the
/// entry's own exported class), gated to what `liveness` reports that
/// module's operations can actually construct for this target.
pub fn taxonomy_and_declared_decls(module: &Module, liveness: &TaxonomyLiveness) -> Vec<Decl> {
    let mut decls = taxonomy_decls(liveness);
    decls.extend(declared_error_decls(module));
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

/// The `Violation` record on its own: the field/constraint/message triple a
/// validator pushes. The full taxonomy embeds it, so it is only emitted standalone
/// for a module that has constraints but no operations (hence no taxonomy).
pub fn violation_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "export interface {} {{\n  field: string;\n  constraint: string;\n  message: string;\n}}",
        n.violation
    ))
}

/// The class name of a declared error: its type name plus an `Error` suffix,
/// so the class never collides with the shape's own data interface in the
/// types file.
fn declared_class_name(err: &DeclaredError) -> String {
    format!("{}Error", error_type_name(err))
}

/// The abstract taxonomy root on its own, so it can anchor the standalone
/// Validation category as well as the full taxonomy.
fn root_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "export abstract class {} extends Error {{\n  retryable(): boolean {{\n    return false;\n  }}\n}}",
        n.root
    ))
}

/// One taxonomy category class extending the root.
fn category_decl(name: &str, ctor_params: &str, message: &str) -> Decl {
    let root = error_names().root;
    Decl::raw(format!(
        "export class {name} extends {root} {{\n  constructor({ctor_params}) {{\n    super({message});\n    this.name = \"{name}\";\n  }}\n}}"
    ))
}

/// The `Validation` category class a validator returns.
fn validation_category_decl() -> Decl {
    let n = error_names();
    category_decl(
        &n.validation,
        &format!("readonly violations: {}[]", n.violation),
        "\"validation failed\"",
    )
}

/// The declarations a validator needs when the module has constraints but no
/// operations (hence no full taxonomy): the root the category descends from,
/// the `Violation` record, and the `Validation` category itself.
pub fn standalone_validation_decls() -> Vec<Decl> {
    vec![root_decl(), violation_decl(), validation_category_decl()]
}

/// The closed error taxonomy: the abstract root, the `Violation` record, and
/// the category classes `liveness` reports as reachable. Unlike Rust's single
/// self-referential enum, each category here is its own class, so gating one
/// out never requires touching another's declaration; only the root is
/// shared, and it is emitted whenever at least one category is live (nothing
/// would extend it otherwise).
fn taxonomy_decls(liveness: &TaxonomyLiveness) -> Vec<Decl> {
    let n = error_names();
    let category = category_decl;
    let any_live = liveness.validation
        || liveness.transport
        || liveness.decode
        || liveness.contract
        || liveness.api
        || liveness.config;
    let mut decls = Vec::new();
    if any_live {
        decls.push(root_decl());
    }
    if liveness.validation {
        decls.push(violation_decl());
        decls.push(validation_category_decl());
    }
    if liveness.transport {
        decls.push(category(
            &n.transport,
            "readonly cause: unknown",
            "\"transport failure\"",
        ));
    }
    if liveness.decode {
        decls.push(category(
            &n.decode,
            "readonly path: string, readonly expected: string, readonly raw: string",
            "\"response body did not match the declared schema\"",
        ));
    }
    if liveness.contract {
        decls.push(category(
            &n.contract,
            "readonly contractName: string, readonly cause: unknown",
            "\"contract hook failed\"",
        ));
    }
    if liveness.api {
        decls.push(category(
            &n.api,
            "readonly status: number, readonly body: string",
            "`api error ${status}`",
        ));
    }
    if liveness.config {
        // Construction failures (a required source that resolved to nothing) ride
        // their own category so a caller can tell a misconfigured client from a
        // request or transport failure.
        decls.push(category(&n.config, "message: string", "message"));
    }
    decls
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
    let fn_name = format!(
        "decode{}Error",
        crate::codegen::conventions::type_ident_from_id(&op.id)
    );
    discriminator_fn_body(&fn_name, ordered, module, n)
}

/// The same discrimination function under a caller-chosen name (an
/// entry-nested operation derives its name through the entry rule, not from
/// the raw shape id).
pub fn discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError], module: &Module) -> Decl {
    discriminator_fn_body(fn_name, ordered, module, &error_names())
}

/// The code-only discrimination a bespoke raw outcome uses. There is no protocol
/// status to match on, so a declared error is chosen by its `@errorCode` alone
/// and everything unmatched resolves to the fallback, whose status 0 marks the
/// absence of a protocol status. An error declared without a code can never be
/// selected here: nothing in the outcome identifies it.
pub fn outcome_discriminator_fn_named(
    fn_name: &str,
    ordered: &[DeclaredError],
    module: &Module,
) -> Decl {
    let n = error_names();
    let fallback = format!("new {}(0, body)", n.api);
    let mut body = format!(
        "  let parsed: any;\n  try {{\n    parsed = JSON.parse(body);\n  }} catch {{\n    return {fallback};\n  }}\n  try {{\n"
    );
    let mut refs: Vec<Symbol> = vec![
        module_symbol(&n.root, module),
        module_symbol(&n.api, module),
    ];
    for err in ordered.iter().filter(|e| e.code.is_some()) {
        let class = declared_class_name(err);
        let data = error_type_name(err);
        let code = err.code.as_deref().unwrap_or_default();
        body.push_str(&format!(
            "    if (code === \"{code}\") {{\n      return new {class}(decode{data}(parsed), body);\n    }}\n"
        ));
        refs.push(module_symbol(&class, module));
    }
    // The declared-error decode can itself throw; an undecodable declared match
    // falls back to the generic type so a new field or a changed shape never
    // breaks the caller.
    body.push_str("  } catch {}\n");
    body.push_str(&format!("  return {fallback};"));
    Decl::Function(Function {
        name: Symbol::builtin(fn_name.to_string()),
        params: vec![string_param("code"), string_param("body")],
        ret: Some(TypeExpr::Ref(module_symbol(&n.root, module))),
        body: FnBody::Raw { text: body, refs },
    })
}

fn string_param(name: &str) -> Field {
    Field {
        name: Symbol::builtin(name.to_string()),
        ty: TypeExpr::Ref(Symbol::builtin("string")),
        nullable: false,
        wire: None,
        deprecated: None,
        doc: None,
    }
}

fn discriminator_fn_body(
    fn_name: &str,
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

    Decl::Function(Function {
        name: Symbol::builtin(fn_name.to_string()),
        params: vec![
            Field {
                name: Symbol::builtin("status"),
                ty: TypeExpr::Ref(Symbol::builtin("number")),
                nullable: false,
                wire: None,
                deprecated: None,
                doc: None,
            },
            string_param("body"),
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
    use crate::codegen::taxonomy::{self, TaxonomyLiveness};
    use crate::codegen::test_support::{error_demo_module, error_shape, operation, rendered};

    /// `error_demo_module`'s own liveness (real, derived, the way a loose TS
    /// operation's `HttpClient` is emitted): a wire operation with declared
    /// errors but no constraints, no entries, and no bound extension, so only
    /// `Transport`/`Decode`/`Api` are reachable.
    fn demo_liveness() -> TaxonomyLiveness {
        taxonomy::derive(&error_demo_module(), &["ts", "typescript"])
    }

    fn types_text(module: &Module) -> String {
        types_text_with(module, &demo_liveness())
    }

    fn types_text_with(module: &Module, liveness: &TaxonomyLiveness) -> String {
        rendered(&type_decls(module, &ts_casing(), liveness), &TsRules)
    }

    #[test]
    fn only_the_categories_the_module_can_construct_extend_the_root() {
        let out = types_text(&error_demo_module());
        assert!(out.contains("export abstract class TonoError extends Error {"));
        for category in ["TransportError", "DecodeError", "APIError"] {
            assert!(
                out.contains(&format!("export class {category} extends TonoError {{")),
                "{category} must extend the root"
            );
        }
        // The root's default predicate reports non-retryable.
        assert!(out.contains("retryable(): boolean {\n    return false;"));
        // No constraints, no entry, no bound extension: Validation, Contract,
        // and Config are all unreachable for this module.
        assert_eq!(out.matches("extends TonoError {").count(), 3);
        assert!(!out.contains("ValidationError"));
        assert!(!out.contains("ContractError"));
        assert!(!out.contains("ConfigError"));
    }

    #[test]
    fn a_module_where_nothing_is_live_gets_no_taxonomy_at_all() {
        let out = types_text_with(
            &error_demo_module(),
            &TaxonomyLiveness {
                validation: false,
                transport: false,
                decode: false,
                api: false,
                config: false,
                contract: false,
            },
        );
        assert!(!out.contains("TonoError"));
    }

    #[test]
    fn the_root_is_emitted_when_exactly_one_category_is_live() {
        // Each case isolates a single true category with every other one
        // false, so the root's presence pins down every operand of the
        // any-live check individually: none of the six can be dropped from it
        // without a module that lights up only that one losing its root.
        let none = TaxonomyLiveness {
            validation: false,
            transport: false,
            decode: false,
            api: false,
            config: false,
            contract: false,
        };
        let cases: [(&str, TaxonomyLiveness); 6] = [
            (
                "validation",
                TaxonomyLiveness {
                    validation: true,
                    ..none
                },
            ),
            (
                "transport",
                TaxonomyLiveness {
                    transport: true,
                    ..none
                },
            ),
            (
                "decode",
                TaxonomyLiveness {
                    decode: true,
                    ..none
                },
            ),
            (
                "contract",
                TaxonomyLiveness {
                    contract: true,
                    ..none
                },
            ),
            ("api", TaxonomyLiveness { api: true, ..none }),
            (
                "config",
                TaxonomyLiveness {
                    config: true,
                    ..none
                },
            ),
        ];
        for (label, liveness) in cases {
            let out = types_text_with(&error_demo_module(), &liveness);
            assert!(
                out.contains("export abstract class TonoError extends Error {"),
                "root must be emitted when only {label} is live"
            );
        }
    }

    #[test]
    fn every_category_extends_the_root_when_everything_is_live() {
        let out = types_text_with(&error_demo_module(), &TaxonomyLiveness::all_live());
        for category in [
            "ValidationError",
            "TransportError",
            "DecodeError",
            "ContractError",
            "APIError",
            "ConfigError",
        ] {
            assert!(
                out.contains(&format!("export class {category} extends TonoError {{")),
                "{category} must extend the root"
            );
        }
        assert_eq!(out.matches("extends TonoError {").count(), 6);
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

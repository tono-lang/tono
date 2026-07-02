//! The Go error surface and client interface: the closed taxonomy as error
//! values (no invented root type, only the stdlib `error` interface), declared
//! operation errors made into error values on their existing structs, the
//! per-operation discrimination function, and the blocking client interface.
//!
//! Go discriminates with `errors.As`, so each category is a distinct struct
//! implementing `error`; the transport and contract categories `Unwrap` their
//! native cause. A declared error stays the struct the types file already
//! emits — the methods added here (`Error`, `Retryable`) are what make it an
//! error value.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::ops::{
    declared_errors, discrimination_order, effect_of, module_declared_errors, DeclaredError, Effect,
};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::go::types::type_expr_of;
use crate::codegen::tree::{ClientDecl, Decl, Field, Method, Raw, TypeExpr};
use crate::ir::{Module, Shape};

/// The canonical taxonomy type names, derived through the same casing engine
/// as every other type identifier (so `api_error` follows the initialism set).
struct Names {
    api: String,
    validation: String,
    transport: String,
    decode: String,
    contract: String,
    violation: String,
}

fn names() -> Names {
    Names {
        api: type_ident_from_id("api_error"),
        validation: type_ident_from_id("validation_error"),
        transport: type_ident_from_id("transport_error"),
        decode: type_ident_from_id("decode_error"),
        contract: type_ident_from_id("contract_error"),
        violation: type_ident_from_id("violation"),
    }
}

fn raw(text: String) -> Decl {
    Decl::Raw(Raw {
        text,
        refs: Vec::new(),
    })
}

fn raw_with(text: String, refs: Vec<Symbol>) -> Decl {
    Decl::Raw(Raw { text, refs })
}

fn json_symbol() -> Symbol {
    Symbol::imported("json", "encoding/json", "json")
}

fn strconv_symbol() -> Symbol {
    Symbol::imported("strconv", "strconv", "strconv")
}

/// The wire message of a declared error: its body code when declared, else its
/// canonical snake name.
fn declared_message(err: &DeclaredError) -> String {
    err.code.clone().unwrap_or_else(|| {
        err.shape_id
            .rsplit('#')
            .next()
            .unwrap_or(&err.shape_id)
            .to_string()
    })
}

/// The closed error taxonomy as error values. Go has no hierarchy to root, so
/// the categories share nothing but the `error` interface; callers pick one
/// with `errors.As`.
pub fn taxonomy_decls() -> Vec<Decl> {
    let n = names();
    vec![
        raw(format!(
            "type {} struct {{\n\tField      string `json:\"field\"`\n\tConstraint string `json:\"constraint\"`\n\tMessage    string `json:\"message\"`\n}}",
            n.violation
        )),
        raw(format!(
            "type {} struct {{\n\tViolations []{} `json:\"violations\"`\n}}",
            n.validation, n.violation
        )),
        raw(format!(
            "func (e *{}) Error() string {{ return \"validation failed\" }}",
            n.validation
        )),
        raw(format!(
            "type {} struct {{\n\tCause error\n}}",
            n.transport
        )),
        raw(format!(
            "func (e *{}) Error() string {{ return \"transport failure\" }}",
            n.transport
        )),
        raw(format!(
            "func (e *{}) Unwrap() error {{ return e.Cause }}",
            n.transport
        )),
        raw(format!(
            "type {} struct {{\n\tPath     string\n\tExpected string\n\tRaw      string\n}}",
            n.decode
        )),
        raw(format!(
            "func (e *{}) Error() string {{ return \"response body did not match the declared schema\" }}",
            n.decode
        )),
        raw(format!(
            "type {} struct {{\n\tContractName string\n\tCause        error\n}}",
            n.contract
        )),
        raw(format!(
            "func (e *{}) Error() string {{ return \"contract hook '\" + e.ContractName + \"' failed\" }}",
            n.contract
        )),
        raw(format!(
            "func (e *{}) Unwrap() error {{ return e.Cause }}",
            n.contract
        )),
        raw(format!(
            "type {} struct {{\n\tStatus int\n\tBody   string\n}}",
            n.api
        )),
        raw_with(
            format!(
                "func (e *{}) Error() string {{ return \"api error \" + strconv.Itoa(e.Status) }}",
                n.api
            ),
            vec![strconv_symbol()],
        ),
    ]
}

/// The methods that make each declared error struct an error value: `Error`
/// (its body code, or its canonical name) and the `Retryable` predicate from
/// `@retryable`.
pub fn declared_error_decls(module: &Module) -> Vec<Decl> {
    module_declared_errors(module)
        .iter()
        .flat_map(|err| {
            let ty = type_ident_from_id(&err.shape_id);
            vec![
                raw(format!(
                    "func (e *{ty}) Error() string {{ return \"{}\" }}",
                    declared_message(err)
                )),
                raw(format!(
                    "func (e *{ty}) Retryable() bool {{ return {} }}",
                    err.retryable
                )),
            ]
        })
        .collect()
}

/// The generated method identifier for an operation.
fn method_ident(op: &Shape, config: &CasingConfig) -> String {
    let local = op.id.rsplit('#').next().unwrap_or(&op.id);
    let rename = crate::codegen::conventions::rename_of(&op.traits, super::types::LANG);
    transform(local, SymbolKind::Method, config, rename.as_deref())
}

fn op_io(op: &Shape) -> (Option<&crate::ir::Tref>, Option<&crate::ir::Tref>) {
    match &op.kind {
        crate::ir::ShapeKind::Operation { input, output, .. } => (input.as_ref(), output.as_ref()),
        _ => (None, None),
    }
}

/// The client interface: one method per operation. Go has no suspension
/// marker, so an async operation lowers to the same blocking signature as a
/// sync one; the error channel is the native `(T, error)` pair.
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
                err: Some(TypeExpr::Ref(Symbol::builtin("error"))),
                // Go lowers async and sync alike; kept for the shared model.
                is_async: effect_of(op) == Effect::Async,
            }
        })
        .collect();
    Decl::Client(ClientDecl {
        name: Symbol::builtin(type_ident_from_id("client")),
        methods,
    })
}

/// The per-operation discrimination functions, one per operation that declares
/// errors: `(status, raw body) -> error`. The mapping tries the declared
/// errors (coded entries before a codeless catch-all on the same status) and
/// resolves everything else to the concrete fallback type.
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
    let fn_name = transform(
        &format!(
            "decode_{}_error",
            op.id.rsplit('#').next().unwrap_or(&op.id)
        ),
        SymbolKind::Method,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    );
    let mut body = String::new();
    body.push_str(&format!(
        "func {fn_name}(status int, body []byte) error {{\n"
    ));
    if ordered.iter().any(|e| e.code.is_some()) {
        body.push_str("\tvar probe struct {\n\t\tCode string `json:\"code\"`\n\t}\n\t_ = json.Unmarshal(body, &probe)\n");
    }
    for err in &ordered {
        let ty = type_ident_from_id(&err.shape_id);
        let status = err.status.unwrap_or(0);
        let guard = match &err.code {
            Some(code) => format!("status == {status} && probe.Code == \"{code}\""),
            None => format!("status == {status}"),
        };
        // A declared match whose body does not unmarshal falls through to the
        // fallback so new server fields or shapes never break the caller.
        body.push_str(&format!(
            "\tif {guard} {{\n\t\tvar data {ty}\n\t\tif json.Unmarshal(body, &data) == nil {{\n\t\t\treturn &data\n\t\t}}\n\t}}\n"
        ));
    }
    body.push_str(&format!(
        "\treturn &{}{{Status: status, Body: string(body)}}\n}}",
        n.api
    ));
    raw_with(body, vec![json_symbol()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
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
        let mut limited = structure("m#rate_limited", vec![]);
        limited.traits = vec![trait_of("status", json!([429]))];
        Module {
            name: "m".into(),
            shapes: vec![
                structure(
                    "m#charge",
                    vec![member("id", Tref::Prim(Prim::String), true)],
                ),
                structure("m#charge_input", vec![]),
                declined,
                limited,
            ],
            operations: vec![op(
                "m#create_charge",
                vec![trait_of(
                    "http",
                    json!({"method": "POST", "path": "/charges"}),
                )],
                vec!["m#payment_declined", "m#rate_limited"],
            )],
        }
    }

    fn rendered(decls: &[Decl]) -> String {
        decls
            .iter()
            .map(|d| GoRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_taxonomy_is_error_values_with_no_invented_root() {
        let out = rendered(&taxonomy_decls());
        for category in [
            "ValidationError",
            "TransportError",
            "DecodeError",
            "ContractError",
            "APIError",
        ] {
            assert!(out.contains(&format!("type {category} struct {{")));
            assert!(
                out.contains(&format!("func (e *{category}) Error() string {{")),
                "{category} must implement error"
            );
        }
        // No root type: nothing named after the hierarchy root is generated.
        assert!(!out.contains("TonoError"));
        // The transport and contract categories unwrap their native cause.
        assert!(out.contains("func (e *TransportError) Unwrap() error { return e.Cause }"));
        assert!(out.contains("func (e *ContractError) Unwrap() error { return e.Cause }"));
        assert!(out.contains("\tStatus int\n\tBody   string\n"));
    }

    #[test]
    fn declared_errors_gain_error_and_retryable_methods() {
        let out = rendered(&declared_error_decls(&demo_module()));
        assert!(out
            .contains("func (e *PaymentDeclined) Error() string { return \"payment_declined\" }"));
        assert!(out.contains("func (e *PaymentDeclined) Retryable() bool { return true }"));
        // Without a code the message falls back to the canonical name; without
        // @retryable the predicate reports false.
        assert!(out.contains("func (e *RateLimited) Error() string { return \"rate_limited\" }"));
        assert!(out.contains("func (e *RateLimited) Retryable() bool { return false }"));
    }

    #[test]
    fn the_client_interface_is_blocking_with_the_error_pair() {
        let out = rendered(&[client_decl(&demo_module(), &go_casing())]);
        assert_eq!(
            out,
            "type Client interface {\n\tCreateCharge(input ChargeInput) (Charge, error)\n}"
        );
    }

    #[test]
    fn the_discriminator_probes_the_code_field_and_falls_back() {
        let out = rendered(&discriminator_decls(&demo_module()));
        assert!(out.contains("func DecodeCreateChargeError(status int, body []byte) error {"));
        assert!(out.contains("Code string `json:\"code\"`"));
        assert!(out.contains("if status == 402 && probe.Code == \"payment_declined\" {"));
        assert!(out.contains("var data PaymentDeclined"));
        assert!(out.contains("if status == 429 {"));
        assert!(out.contains("return &APIError{Status: status, Body: string(body)}"));
    }

    #[test]
    fn a_codeless_error_set_skips_the_probe() {
        let mut module = demo_module();
        module.operations = vec![op("m#fetch", vec![], vec!["m#rate_limited"])];
        let out = rendered(&discriminator_decls(&module));
        assert!(!out.contains("probe"));
        assert!(out.contains("if status == 429 {"));
    }

    #[test]
    fn an_operation_with_no_declared_errors_gets_no_discriminator() {
        let mut module = demo_module();
        module.operations = vec![op("m#ping", vec![], vec![])];
        assert!(discriminator_decls(&module).is_empty());
    }
}

//! The Go error surface and client interface: the closed taxonomy as error
//! values (no invented root type, only the stdlib `error` interface), declared
//! operation errors made into error values on their existing structs, the
//! per-operation discrimination function, and the blocking client interface.
//!
//! Go discriminates with `errors.As`, so each category is a distinct struct
//! implementing `error`; the transport and contract categories `Unwrap` their
//! native cause. A declared error stays the struct the types file already
//! emits, the methods added here (`Error`, `Retryable`) are what make it an
//! error value.

use crate::codegen::casing::CasingConfig;
use crate::codegen::ops::{
    self, error_names, error_type_name, module_declared_errors, DeclaredError, ErrorNames,
};
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::go::types::{type_expr_of, LANG};
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape};

/// The declarations for the types file: the taxonomy error values, the
/// declared errors' methods, and the blocking client interface.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let mut decls = taxonomy_decls();
    decls.extend(declared_error_decls(module));
    // The error channel is the native (T, error) pair on every method.
    decls.push(ops::client_decl(
        module,
        config,
        LANG,
        &type_expr_of,
        Some("error"),
    ));
    decls
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| discriminator_fn(op, ordered, &n))
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
fn taxonomy_decls() -> Vec<Decl> {
    let n = error_names();
    let error_method = |name: &str, message: &str| {
        Decl::raw(format!(
            "func (e *{name}) Error() string {{ return {message} }}"
        ))
    };
    let unwrap_method = |name: &str| {
        Decl::raw(format!(
            "func (e *{name}) Unwrap() error {{ return e.Cause }}"
        ))
    };
    vec![
        Decl::raw(format!(
            "type {} struct {{\n\tField      string `json:\"field\"`\n\tConstraint string `json:\"constraint\"`\n\tMessage    string `json:\"message\"`\n}}",
            n.violation
        )),
        Decl::raw(format!(
            "type {} struct {{\n\tViolations []{} `json:\"violations\"`\n}}",
            n.validation, n.violation
        )),
        error_method(&n.validation, "\"validation failed\""),
        Decl::raw(format!("type {} struct {{\n\tCause error\n}}", n.transport)),
        error_method(&n.transport, "\"transport failure\""),
        unwrap_method(&n.transport),
        Decl::raw(format!(
            "type {} struct {{\n\tPath     string\n\tExpected string\n\tRaw      string\n}}",
            n.decode
        )),
        error_method(
            &n.decode,
            "\"response body did not match the declared schema\"",
        ),
        Decl::raw(format!(
            "type {} struct {{\n\tContractName string\n\tCause        error\n}}",
            n.contract
        )),
        error_method(
            &n.contract,
            "\"contract hook '\" + e.ContractName + \"' failed\"",
        ),
        unwrap_method(&n.contract),
        Decl::raw(format!(
            "type {} struct {{\n\tStatus int\n\tBody   string\n}}",
            n.api
        )),
        Decl::raw_with(
            format!(
                "func (e *{}) Error() string {{ return \"api error \" + strconv.Itoa(e.Status) }}",
                n.api
            ),
            vec![Symbol::imported("strconv", "strconv", "strconv")],
        ),
    ]
}

/// The methods that make each declared error struct an error value: `Error`
/// (its body code, or its canonical name) and the `Retryable` predicate from
/// `@retryable`.
fn declared_error_decls(module: &Module) -> Vec<Decl> {
    module_declared_errors(module)
        .iter()
        .flat_map(|err| {
            let ty = error_type_name(err);
            vec![
                Decl::raw(format!(
                    "func (e *{ty}) Error() string {{ return \"{}\" }}",
                    declared_message(err)
                )),
                Decl::raw(format!(
                    "func (e *{ty}) Retryable() bool {{ return {} }}",
                    err.retryable
                )),
            ]
        })
        .collect()
}

/// One discrimination function: `(status, raw body) -> error`. The mapping
/// tries the declared errors and resolves everything else to the concrete
/// fallback type.
fn discriminator_fn(op: &Shape, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let fn_name = format!(
        "Decode{}Error",
        crate::codegen::conventions::type_ident_from_id(&op.id)
    );
    let mut body = String::new();
    body.push_str(&format!(
        "func {fn_name}(status int, body []byte) error {{\n"
    ));
    if ordered.iter().any(|e| e.code.is_some()) {
        body.push_str(
            "\tvar probe struct {\n\t\tCode string `json:\"code\"`\n\t}\n\t_ = json.Unmarshal(body, &probe)\n",
        );
    }
    for err in ordered {
        let ty = error_type_name(err);
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
    Decl::raw_with(
        body,
        vec![Symbol::imported("json", "encoding/json", "json")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{error_demo_module, error_shape, operation, rendered};

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &go_casing()), &GoRules)
    }

    #[test]
    fn the_taxonomy_is_error_values_with_no_invented_root() {
        let out = types_text(&error_demo_module());
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
        let out = types_text(&error_demo_module());
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
        let out = types_text(&error_demo_module());
        assert!(out.contains(
            "type Client interface {\n\tCreateCharge(input ChargeInput) (Charge, error)\n}"
        ));
    }

    #[test]
    fn the_discriminator_probes_the_code_field_and_falls_back() {
        let out = rendered(&serde_decls(&error_demo_module()), &GoRules);
        assert!(out.contains("func DecodeCreateChargeError(status int, body []byte) error {"));
        assert!(out.contains("Code string `json:\"code\"`"));
        assert!(out.contains("if status == 402 && probe.Code == \"payment_declined\" {"));
        assert!(out.contains("var data PaymentDeclined"));
        assert!(out.contains("if status == 429 {"));
        assert!(out.contains("return &APIError{Status: status, Body: string(body)}"));
    }

    #[test]
    fn a_codeless_error_set_skips_the_probe() {
        let mut module = error_demo_module();
        module
            .shapes
            .push(error_shape("m#slow_down", vec![], 503, None, false));
        module.operations = vec![operation("m#fetch", vec![], vec!["m#slow_down"])];
        let out = rendered(&serde_decls(&module), &GoRules);
        assert!(!out.contains("probe"));
        assert!(out.contains("if status == 503 {"));
    }
}

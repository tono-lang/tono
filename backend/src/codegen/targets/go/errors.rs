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

/// The anonymous marker interface a bound hook's boundary wrapper matches with
/// `errors.As` to tell an SDK-emitted error (preserve it as-is) from a foreign
/// one (wrap it as a ContractError). Every generated error value carries the
/// unexported `sdkError()` method, so only this package's types satisfy it and
/// the taxonomy stays sealed.
pub const SDK_ERROR_MARKER: &str = "interface{ sdkError() }";

/// The unexported marker method that makes a generated error value part of the
/// sealed SDK taxonomy (see [`SDK_ERROR_MARKER`]).
fn marker_method(name: &str) -> Decl {
    Decl::raw(format!("func (e *{name}) sdkError() {{}}"))
}

/// The declarations for the types file: the taxonomy error values, the
/// declared errors' methods, and the blocking client interface.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let mut decls = taxonomy_and_declared_decls(module);
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

/// The taxonomy and the declared errors' methods without the loose-op client
/// interface: what an entry-only module needs (its client surface is the
/// entry's own struct and mock interface).
pub fn taxonomy_and_declared_decls(module: &Module) -> Vec<Decl> {
    let mut decls = taxonomy_decls();
    decls.extend(declared_error_decls(module));
    decls
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| discriminator_fn(op, ordered, &n))
}

/// The `Violation` record on its own: the field/constraint/message triple a
/// validator appends. The full taxonomy embeds it, so it is only emitted standalone
/// for a module that has constraints but no operations (hence no taxonomy).
pub fn violation_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "type {} struct {{\n\tField      string `json:\"field\"`\n\tConstraint string `json:\"constraint\"`\n\tMessage    string `json:\"message\"`\n}}",
        n.violation
    ))
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

/// The `Validation` category a validator returns: the error struct carrying its
/// collected violations, the `Error` method that makes it an error value, and the
/// marker that keeps it inside the sealed SDK taxonomy.
fn validation_category_decls() -> Vec<Decl> {
    let n = error_names();
    vec![
        Decl::raw(format!(
            "type {} struct {{\n\tViolations []{} `json:\"violations\"`\n}}",
            n.validation, n.violation
        )),
        Decl::raw(format!(
            "func (e *{}) Error() string {{ return \"validation failed\" }}",
            n.validation
        )),
        marker_method(&n.validation),
    ]
}

/// The declarations a validator needs when the module has constraints but no
/// operations (hence no full taxonomy): the `Violation` record and the
/// `Validation` category itself.
pub fn standalone_validation_decls() -> Vec<Decl> {
    let mut decls = vec![violation_decl()];
    decls.extend(validation_category_decls());
    decls
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
    let mut decls = standalone_validation_decls();
    decls.extend(vec![
        Decl::raw(format!("type {} struct {{\n\tCause error\n}}", n.transport)),
        error_method(&n.transport, "\"transport failure\""),
        unwrap_method(&n.transport),
        marker_method(&n.transport),
        Decl::raw(format!(
            "type {} struct {{\n\tPath     string\n\tExpected string\n\tRaw      string\n}}",
            n.decode
        )),
        error_method(
            &n.decode,
            "\"response body did not match the declared schema\"",
        ),
        marker_method(&n.decode),
        Decl::raw(format!(
            "type {} struct {{\n\tContractName string\n\tCause        error\n}}",
            n.contract
        )),
        error_method(
            &n.contract,
            "\"contract hook '\" + e.ContractName + \"' failed\"",
        ),
        unwrap_method(&n.contract),
        marker_method(&n.contract),
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
        marker_method(&n.api),
        // Construction failures (a required source that resolved to nothing) ride
        // their own category so a caller can tell a misconfigured client from a
        // request or transport failure.
        Decl::raw(format!("type {} struct {{\n\tMessage string\n}}", n.config)),
        error_method(&n.config, "e.Message"),
        marker_method(&n.config),
    ]);
    decls
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
                marker_method(&ty),
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
    discriminator_fn_body(&fn_name, ordered, n)
}

/// The same discrimination function under a caller-chosen name (an
/// entry-nested operation derives its name through the entry rule, not from
/// the raw shape id).
pub fn discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    discriminator_fn_body(fn_name, ordered, &error_names())
}

/// The code-only discrimination a bespoke raw outcome uses. There is no protocol
/// status to match on, so a declared error is chosen by its `@errorCode` alone
/// and everything unmatched resolves to the fallback, whose status 0 marks the
/// absence of a protocol status. An error declared without a code can never be
/// selected here: nothing in the outcome identifies it.
pub fn outcome_discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    let n = error_names();
    let mut body = format!("func {fn_name}(code string, body []byte) error {{\n");
    for err in ordered.iter().filter(|e| e.code.is_some()) {
        let ty = error_type_name(err);
        let code = err.code.as_deref().unwrap_or_default();
        // A declared match whose body does not unmarshal falls through to the
        // fallback so a new field or a changed shape never breaks the caller.
        body.push_str(&format!(
            "\tif code == \"{code}\" {{\n\t\tvar data {ty}\n\t\tif json.Unmarshal(body, &data) == nil {{\n\t\t\treturn &data\n\t\t}}\n\t}}\n"
        ));
    }
    body.push_str(&format!(
        "\treturn &{}{{Status: 0, Body: string(body)}}\n}}",
        n.api
    ));
    Decl::raw_with(
        body,
        vec![Symbol::imported("json", "encoding/json", "json")],
    )
}

fn discriminator_fn_body(fn_name: &str, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
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
        rendered(&type_decls(module, &go_casing()), &GoRules::default())
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
        let out = rendered(&serde_decls(&error_demo_module()), &GoRules::default());
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
        let out = rendered(&serde_decls(&module), &GoRules::default());
        assert!(!out.contains("probe"));
        assert!(out.contains("if status == 503 {"));
    }
}

//! The Rust error surface and client trait: the closed taxonomy as `enum
//! TonoError` (exactly one variant per category, no inheritance), declared
//! operation errors as variants of the `Api` payload enum, the per-operation
//! discrimination function, and the client trait whose async operations are
//! `async fn`s returning `Result`.
//!
//! Rust discriminates by `match`, so the `Api` category is itself an enum: one
//! variant per declared error plus `Undeclared` carrying the concrete fallback
//! type. That keeps the category (the enum) and the fallback type (the
//! `{status, body}` struct) distinct while a single match arm still covers the
//! whole category.

use crate::codegen::casing::CasingConfig;
use crate::codegen::ops::{
    self, error_names, error_type_name, module_declared_errors, DeclaredError, ErrorNames,
};
use crate::codegen::targets::rust::types::{type_expr_of, LANG};
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape};

/// The declarations for the types file: the taxonomy with the module's `Api`
/// payload enum, and the client trait.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let n = error_names();
    let mut decls = taxonomy_decls(module, &n);
    // The error channel is the Result idiom, so every client method returns
    // `Result<_, TonoError>`.
    decls.push(ops::client_decl(
        module,
        config,
        LANG,
        &type_expr_of,
        Some(&n.root),
    ));
    decls
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| discriminator_fn(op, ordered, &n))
}

/// The closed error taxonomy plus the module's `Api` payload enum. The
/// category structs are plain data; the `TonoError` enum carries exactly one
/// variant per category and implements `Display`/`Error` (with the transport
/// and contract causes as sources) and the retryable predicate.
fn taxonomy_decls(module: &Module, n: &ErrorNames) -> Vec<Decl> {
    let declared = module_declared_errors(module);
    let data_struct = |name: &str, fields: &str| {
        Decl::raw(format!(
            "#[derive(Debug)]\npub struct {name} {{\n{fields}}}"
        ))
    };
    let mut decls = vec![
        data_struct(
            &n.violation,
            "    pub field: String,\n    pub constraint: String,\n    pub message: String,\n",
        ),
        data_struct(
            &n.validation,
            &format!("    pub violations: Vec<{}>,\n", n.violation),
        ),
        data_struct(
            &n.transport,
            "    pub cause: Box<dyn std::error::Error + Send + Sync>,\n",
        ),
        data_struct(
            &n.decode,
            "    pub path: String,\n    pub expected: String,\n    pub raw: String,\n",
        ),
        data_struct(
            &n.contract,
            "    pub contract_name: String,\n    pub cause: Box<dyn std::error::Error + Send + Sync>,\n",
        ),
        data_struct(&n.api, "    pub status: u16,\n    pub body: String,\n"),
    ];

    let mut failure_variants: Vec<String> = declared
        .iter()
        .map(|err| format!("    {name}({name}),\n", name = error_type_name(err)))
        .collect();
    failure_variants.push(format!("    Undeclared({}),\n", n.api));
    decls.push(Decl::raw(format!(
        "#[derive(Debug)]\npub enum {} {{\n{}}}",
        n.api_failure,
        failure_variants.concat()
    )));

    let retryable_arms: String = declared
        .iter()
        .filter(|err| err.retryable)
        .map(|err| {
            format!(
                "            {}::{}(_) => true,\n",
                n.api_failure,
                error_type_name(err)
            )
        })
        .collect();
    let failure_retryable_body = if retryable_arms.is_empty() {
        "        false".to_string()
    } else {
        format!("        match self {{\n{retryable_arms}            _ => false,\n        }}")
    };
    decls.push(Decl::raw(format!(
        "impl {} {{\n    pub fn retryable(&self) -> bool {{\n{failure_retryable_body}\n    }}\n}}",
        n.api_failure
    )));

    decls.push(Decl::raw(format!(
        "#[derive(Debug)]\npub enum {root} {{\n    Validation({validation}),\n    Transport({transport}),\n    Api({api_failure}),\n    Decode({decode}),\n    Contract({contract}),\n}}",
        root = n.root,
        validation = n.validation,
        transport = n.transport,
        api_failure = n.api_failure,
        decode = n.decode,
        contract = n.contract,
    )));

    decls.push(Decl::raw(format!(
        "impl {root} {{\n    pub fn retryable(&self) -> bool {{\n        match self {{\n            {root}::Api(failure) => failure.retryable(),\n            _ => false,\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls.push(Decl::raw(format!(
        "impl std::fmt::Display for {root} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        match self {{\n            {root}::Validation(_) => write!(f, \"validation failed\"),\n            {root}::Transport(_) => write!(f, \"transport failure\"),\n            {root}::Api(_) => write!(f, \"api error\"),\n            {root}::Decode(_) => write!(f, \"response body did not match the declared schema\"),\n            {root}::Contract(e) => write!(f, \"contract hook '{{}}' failed\", e.contract_name),\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls.push(Decl::raw(format!(
        "impl std::error::Error for {root} {{\n    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {{\n        match self {{\n            {root}::Transport(e) => Some(e.cause.as_ref()),\n            {root}::Contract(e) => Some(e.cause.as_ref()),\n            _ => None,\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls
}

/// One discrimination function: `(status, raw body) -> TonoError`. The mapping
/// tries the declared errors and resolves everything else to `Undeclared`
/// carrying the concrete fallback type.
fn discriminator_fn(op: &Shape, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let fallback = format!(
        "{root}::Api({failure}::Undeclared({api} {{ status, body: body.to_string() }}))",
        root = n.root,
        failure = n.api_failure,
        api = n.api
    );
    let fn_name = format!(
        "decode_{}_error",
        op.id.rsplit('#').next().unwrap_or(&op.id)
    );
    let mut body = String::new();
    body.push_str(&format!(
        "pub fn {fn_name}(status: u16, body: &str) -> {} {{\n",
        n.root
    ));
    body.push_str(&format!(
        "    let value: serde_json::Value = match serde_json::from_str(body) {{\n        Ok(value) => value,\n        Err(_) => return {fallback},\n    }};\n"
    ));
    if ordered.iter().any(|e| e.code.is_some()) {
        body.push_str("    let code = value.get(\"code\").and_then(|v| v.as_str());\n");
    }
    for err in ordered {
        let data = error_type_name(err);
        let status = err.status.unwrap_or(0);
        let guard = match &err.code {
            Some(code) => format!("status == {status} && code == Some(\"{code}\")"),
            None => format!("status == {status}"),
        };
        // A declared match whose body does not decode falls through to the
        // fallback so new server fields or shapes never break the caller.
        body.push_str(&format!(
            "    if {guard} {{\n        if let Ok(data) = serde_json::from_value::<{data}>(value.clone()) {{\n            return {root}::Api({failure}::{data}(data));\n        }}\n    }}\n",
            root = n.root,
            failure = n.api_failure,
        ));
    }
    body.push_str(&format!("    {fallback}\n}}"));
    Decl::raw(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::{error_demo_module, operation, rendered};

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &rust_casing()), &RustRules)
    }

    #[test]
    fn the_root_is_an_enum_with_exactly_one_variant_per_category() {
        let out = types_text(&error_demo_module());
        assert!(out.contains("pub enum TonoError {"));
        for arm in [
            "Validation(ValidationError),",
            "Transport(TransportError),",
            "Api(APIFailure),",
            "Decode(DecodeError),",
            "Contract(ContractError),",
        ] {
            assert!(out.contains(arm), "missing category arm {arm}");
        }
        // The category structs carry the canonical fields.
        assert!(
            out.contains("pub struct APIError {\n    pub status: u16,\n    pub body: String,\n}")
        );
        assert!(out.contains("pub violations: Vec<Violation>,"));
        assert!(out.contains("pub cause: Box<dyn std::error::Error + Send + Sync>,"));
        assert!(out.contains("pub path: String,"));
        assert!(out.contains("pub contract_name: String,"));
        // Display and Error are implemented on the root.
        assert!(out.contains("impl std::fmt::Display for TonoError {"));
        assert!(out.contains("impl std::error::Error for TonoError {"));
    }

    #[test]
    fn declared_errors_become_api_payload_variants_next_to_the_fallback() {
        let out = types_text(&error_demo_module());
        assert!(out.contains("pub enum APIFailure {"));
        assert!(out.contains("    PaymentDeclined(PaymentDeclined),"));
        assert!(out.contains("    RateLimited(RateLimited),"));
        assert!(out.contains("    Undeclared(APIError),"));
        // @retryable lowers into the predicate; the root delegates to the
        // Api payload and reports false everywhere else.
        assert!(out.contains("APIFailure::PaymentDeclined(_) => true,"));
        assert!(out.contains("TonoError::Api(failure) => failure.retryable(),"));
    }

    #[test]
    fn a_module_with_no_retryable_error_gets_a_constant_predicate() {
        let mut module = error_demo_module();
        for shape in &mut module.shapes {
            shape.traits.retain(|t| t.id != "retryable");
        }
        let out = types_text(&module);
        assert!(out.contains("pub fn retryable(&self) -> bool {\n        false\n    }"));
    }

    #[test]
    fn the_client_trait_lowers_the_effect_to_async_fn_returning_result() {
        let out = types_text(&error_demo_module());
        assert!(out.contains("#[allow(async_fn_in_trait)]"));
        assert!(out.contains(
            "    async fn create_charge(&self, input: ChargeInput) -> Result<Charge, TonoError>;"
        ));
    }

    #[test]
    fn a_sync_operation_keeps_a_plain_fn() {
        let mut module = error_demo_module();
        module.operations = vec![operation("m#local_sum", vec![], vec![])];
        let out = types_text(&module);
        assert!(out
            .contains("    fn local_sum(&self, input: ChargeInput) -> Result<Charge, TonoError>;"));
        assert!(!out.contains("async fn"));
    }

    #[test]
    fn the_discriminator_matches_status_and_code_then_falls_back() {
        let out = rendered(&serde_decls(&error_demo_module()), &RustRules);
        assert!(out
            .contains("pub fn decode_create_charge_error(status: u16, body: &str) -> TonoError {"));
        assert!(out.contains("if status == 402 && code == Some(\"payment_declined\") {"));
        assert!(out.contains("serde_json::from_value::<PaymentDeclined>(value.clone())"));
        assert!(out.contains("return TonoError::Api(APIFailure::PaymentDeclined(data));"));
        assert!(out.contains("if status == 429 {"));
        assert!(out.contains(
            "TonoError::Api(APIFailure::Undeclared(APIError { status, body: body.to_string() }))"
        ));
    }
}

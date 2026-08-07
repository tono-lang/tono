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
use crate::codegen::taxonomy::TaxonomyLiveness;
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape};

/// The declarations for the types file: the taxonomy with the module's `Api`
/// payload enum, and the client trait.
///
/// A loose (non-entry) operation gets only a client *trait* (no concrete
/// client), so nothing in the generated SDK constructs any category either
/// way; the full taxonomy is vocabulary the trait implementer may need, not
/// dead code, hence [`TaxonomyLiveness::all_live`] rather than a derived
/// liveness.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let n = error_names();
    let mut decls = taxonomy_decls(module, &n, &TaxonomyLiveness::all_live());
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

/// The taxonomy (with the module's Api payload enum) without the loose-op
/// client trait: what a module whose operations live in an entry needs. The
/// entry's own construction surface (its builder, its op methods) lives in
/// its own group (`rust/entry`); this only carries the error types its
/// methods and its consumers match on, gated to what `liveness` reports that
/// module's entries can actually construct for this target.
pub fn taxonomy_and_declared_decls(module: &Module, liveness: &TaxonomyLiveness) -> Vec<Decl> {
    taxonomy_decls(module, &error_names(), liveness)
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| discriminator_fn(op, ordered, &n))
}

/// The `Violation` record on its own: the field/constraint/message triple a
/// validator pushes. The full taxonomy embeds it, so it is only emitted standalone
/// for a module that has constraints but no operations (hence no taxonomy).
pub fn violation_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "#[derive(Debug)]\npub struct {} {{\n    pub field: String,\n    pub constraint: String,\n    pub message: String,\n}}",
        n.violation
    ))
}

/// The `Validation` category struct a validator returns, carrying its
/// collected violations.
fn validation_category_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "#[derive(Debug)]\npub struct {} {{\n    pub violations: Vec<{}>,\n}}",
        n.validation, n.violation
    ))
}

/// The declarations a validator needs when the module has constraints but no
/// operations (hence no full taxonomy): the `Violation` record and the
/// `Validation` category itself.
pub fn standalone_validation_decls() -> Vec<Decl> {
    vec![violation_decl(), validation_category_decl()]
}

/// The closed error taxonomy plus the module's `Api` payload enum, gated to
/// the categories `liveness` reports as reachable: a category nothing
/// generated for this module can construct does not get a struct, a
/// `TonoError` variant, or a `Display`/`Error` arm. The taxonomy is
/// self-referential (its own `Display`/`Error` impls name every category), so
/// this has to be built conditionally rather than pruned after the fact.
fn taxonomy_decls(module: &Module, n: &ErrorNames, liveness: &TaxonomyLiveness) -> Vec<Decl> {
    let declared = module_declared_errors(module);
    let data_struct = |name: &str, fields: &str| {
        Decl::raw_providing(
            name,
            format!("#[derive(Debug)]\npub struct {name} {{\n{fields}}}"),
            vec![],
        )
    };
    let mut decls = Vec::new();
    if liveness.validation {
        decls.push(violation_decl());
        decls.push(validation_category_decl());
    }
    if liveness.transport {
        decls.push(data_struct(
            &n.transport,
            "    pub cause: Box<dyn std::error::Error + Send + Sync>,\n",
        ));
    }
    if liveness.decode {
        // `path` is a manual top-level required-member probe against
        // `serde_json::Value` (see `entry/decode.rs::success_block`), the same
        // hand-written shape Go/TS use; it does not descend into nested
        // structures. `serde_path_to_error` was considered and rejected: it
        // would add a dependency to every generated Rust SDK to solve a
        // nested-path case none of the other targets solve today either, so
        // the fidelity gap is deliberate and symmetric across targets, not a
        // Rust-specific shortcut.
        decls.push(data_struct(
            &n.decode,
            "    pub path: String,\n    pub expected: String,\n    pub raw: String,\n",
        ));
    }
    if liveness.contract {
        decls.push(data_struct(
            &n.contract,
            "    pub contract_name: String,\n    pub cause: Box<dyn std::error::Error + Send + Sync>,\n",
        ));
    }
    if liveness.api {
        decls.push(data_struct(
            &n.api,
            "    pub status: u16,\n    pub body: String,\n",
        ));
    }
    if liveness.config {
        // Construction failures (a required source that resolved to nothing)
        // ride their own category so a caller can tell a misconfigured client
        // from a request or transport failure.
        decls.push(data_struct(&n.config, "    pub message: String,\n"));
    }

    if liveness.api {
        let mut failure_variants: Vec<String> = declared
            .iter()
            .map(|err| format!("    {name}({name}),\n", name = error_type_name(err)))
            .collect();
        failure_variants.push(format!("    Undeclared({}),\n", n.api));
        decls.push(Decl::raw_providing(
            &n.api_failure,
            format!(
                "#[derive(Debug)]\npub enum {} {{\n{}}}",
                n.api_failure,
                failure_variants.concat()
            ),
            vec![],
        ));

        // `matches!` over an or-pattern rather than a hand-written match: a
        // `X(_) => true, _ => false` match is exactly what the generated
        // SDK's own match-like-matches lint rewrites to this.
        let retryable_patterns: Vec<String> = declared
            .iter()
            .filter(|err| err.retryable)
            .map(|err| format!("{}::{}(_)", n.api_failure, error_type_name(err)))
            .collect();
        let failure_retryable_body = if retryable_patterns.is_empty() {
            "        false".to_string()
        } else {
            format!("        matches!(self, {})", retryable_patterns.join(" | "))
        };
        decls.push(Decl::raw(format!(
            "impl {} {{\n    pub fn retryable(&self) -> bool {{\n{failure_retryable_body}\n    }}\n}}",
            n.api_failure
        )));
    }

    let mut root_variants = Vec::new();
    let mut display_arms = Vec::new();
    if liveness.validation {
        root_variants.push(format!("    Validation({}),\n", n.validation));
        display_arms.push(format!(
            "            {root}::Validation(_) => write!(f, \"validation failed\"),\n",
            root = n.root
        ));
    }
    if liveness.transport {
        root_variants.push(format!("    Transport({}),\n", n.transport));
        display_arms.push(format!(
            "            {root}::Transport(_) => write!(f, \"transport failure\"),\n",
            root = n.root
        ));
    }
    if liveness.api {
        root_variants.push(format!("    Api({}),\n", n.api_failure));
        display_arms.push(format!(
            "            {root}::Api(_) => write!(f, \"api error\"),\n",
            root = n.root
        ));
    }
    if liveness.decode {
        root_variants.push(format!("    Decode({}),\n", n.decode));
        display_arms.push(format!(
            "            {root}::Decode(_) => write!(f, \"response body did not match the declared schema\"),\n",
            root = n.root
        ));
    }
    if liveness.contract {
        root_variants.push(format!("    Contract({}),\n", n.contract));
        display_arms.push(format!(
            "            {root}::Contract(e) => write!(f, \"contract hook '{{}}' failed\", e.contract_name),\n",
            root = n.root
        ));
    }
    if liveness.config {
        root_variants.push(format!("    Config({}),\n", n.config));
        display_arms.push(format!(
            "            {root}::Config(e) => write!(f, \"{{}}\", e.message),\n",
            root = n.root
        ));
    }

    decls.push(Decl::raw_providing(
        &n.root,
        format!(
            "#[derive(Debug)]\npub enum {root} {{\n{variants}}}",
            root = n.root,
            variants = root_variants.concat()
        ),
        vec![],
    ));

    let retryable_body = if liveness.api {
        format!(
            "        match self {{\n            {root}::Api(failure) => failure.retryable(),\n            _ => false,\n        }}",
            root = n.root
        )
    } else {
        "        false".to_string()
    };
    decls.push(Decl::raw(format!(
        "impl {root} {{\n    pub fn retryable(&self) -> bool {{\n{retryable_body}\n    }}\n}}",
        root = n.root
    )));

    decls.push(Decl::raw(format!(
        "impl std::fmt::Display for {root} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        match self {{\n{arms}        }}\n    }}\n}}",
        root = n.root,
        arms = display_arms.concat()
    )));

    let mut source_arms = Vec::new();
    if liveness.transport {
        source_arms.push(format!(
            "            {root}::Transport(e) => Some(e.cause.as_ref()),\n",
            root = n.root
        ));
    }
    if liveness.contract {
        source_arms.push(format!(
            "            {root}::Contract(e) => Some(e.cause.as_ref()),\n",
            root = n.root
        ));
    }
    let source_body = if source_arms.is_empty() {
        "        None".to_string()
    } else {
        format!(
            "        match self {{\n{arms}            _ => None,\n        }}",
            arms = source_arms.concat()
        )
    };
    decls.push(Decl::raw(format!(
        "impl std::error::Error for {root} {{\n    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {{\n{source_body}\n    }}\n}}",
        root = n.root
    )));

    decls
}

/// One discrimination function: `(status, raw body) -> TonoError`. The mapping
/// tries the declared errors and resolves everything else to `Undeclared`
/// carrying the concrete fallback type.
fn discriminator_fn(op: &Shape, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let fn_name = format!(
        "decode_{}_error",
        op.id.rsplit('#').next().unwrap_or(&op.id)
    );
    discriminator_fn_body(&fn_name, ordered, n)
}

/// The same discrimination function under a caller-chosen name (an
/// entry-nested operation derives its name through the entry rule, not from
/// the raw shape id, and a multi-entry module prefixes it).
pub fn discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    discriminator_fn_body(fn_name, ordered, &error_names())
}

/// The code-only discrimination a bespoke raw outcome uses. There is no
/// protocol status to match on, so a declared error is chosen by its
/// `@errorCode` alone and everything unmatched resolves to the fallback,
/// whose status 0 marks the absence of a protocol status. An error declared
/// without a code can never be selected here: nothing in the outcome
/// identifies it.
pub fn outcome_discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    let n = error_names();
    let mut body = format!(
        "pub fn {fn_name}(code: Option<&str>, body: &str) -> {root} {{\n",
        root = n.root
    );
    for err in ordered.iter().filter(|e| e.code.is_some()) {
        let data = error_type_name(err);
        let declared_code = err
            .code
            .as_ref()
            .map(|c| c.value.as_str())
            .unwrap_or_default();
        // A declared match whose body does not decode falls through to the
        // fallback so a new field or a changed shape never breaks the caller.
        body.push_str(&format!(
            "    if code == Some({declared_code:?}) {{\n        if let Ok(data) = serde_json::from_str::<{data}>(body) {{\n            return {root}::Api({failure}::{data}(data));\n        }}\n    }}\n",
            root = n.root,
            failure = n.api_failure,
        ));
    }
    body.push_str(&format!(
        "    {root}::Api({failure}::Undeclared({api} {{ status: 0, body: body.to_string() }}))\n}}",
        root = n.root,
        failure = n.api_failure,
        api = n.api,
    ));
    Decl::raw(body)
}

/// The JSON Pointer (RFC 6901) for a declared error's discriminator path:
/// `["error", "type"]` becomes `/error/type`. A segment's own `~` and `/` are
/// escaped per the pointer syntax, though neither appears in a canonical
/// snake_case path in practice.
fn json_pointer(path: &[String]) -> String {
    path.iter()
        .map(|seg| format!("/{}", seg.replace('~', "~0").replace('/', "~1")))
        .collect()
}

fn discriminator_fn_body(fn_name: &str, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let fallback = format!(
        "{root}::Api({failure}::Undeclared({api} {{ status, body: body.to_string() }}))",
        root = n.root,
        failure = n.api_failure,
        api = n.api
    );
    let mut body = String::new();
    body.push_str(&format!(
        "pub fn {fn_name}(status: u16, body: &str) -> {} {{\n",
        n.root
    ));
    // The body's JSON is the wire contract: a body that fails to decode here
    // never reaches a guard below, so it falls straight to the generic
    // fallback carrying the raw body.
    body.push_str(&format!(
        "    let value: serde_json::Value = match serde_json::from_str(body) {{\n        Ok(value) => value,\n        Err(_) => return {fallback},\n    }};\n"
    ));
    for err in ordered {
        let data = error_type_name(err);
        let status = err.status.unwrap_or(0);
        let guard = match &err.code {
            Some(code) => format!(
                "status == {status} && value.pointer({pointer:?}).and_then(|v| v.as_str()) == Some({value:?})",
                pointer = json_pointer(&code.path),
                value = code.value,
            ),
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
    use crate::codegen::ops::ErrorCode;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::{error_demo_module, operation, rendered};

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &rust_casing()), &RustRules::default())
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
    fn outcome_discriminator_matches_by_code_alone_and_falls_back_to_status_zero() {
        let ordered = vec![
            DeclaredError {
                shape_id: "m#quota_exceeded".into(),
                status: Some(429),
                code: Some(ErrorCode {
                    path: vec!["code".into()],
                    value: "quota_exceeded".into(),
                }),
                retryable: false,
            },
            // A declared error with no code can never be selected here
            // (nothing in a raw outcome identifies it), so it contributes no
            // arm to the generated function.
            DeclaredError {
                shape_id: "m#opaque_failure".into(),
                status: Some(500),
                code: None,
                retryable: false,
            },
        ];
        let out = rendered(
            &[outcome_discriminator_fn_named(
                "decode_save_outcome",
                &ordered,
            )],
            &RustRules::default(),
        );
        assert!(out
            .contains("pub fn decode_save_outcome(code: Option<&str>, body: &str) -> TonoError {"));
        assert!(out.contains("if code == Some(\"quota_exceeded\") {"));
        assert!(out.contains("serde_json::from_str::<QuotaExceeded>(body)"));
        assert!(!out.contains("OpaqueFailure"));
        assert!(out.contains("TonoError::Api(APIFailure::Undeclared(APIError { status: 0, body: body.to_string() }))"));
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
        assert!(out.contains("matches!(self, APIFailure::PaymentDeclined(_))"));
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
        let out = rendered(&serde_decls(&error_demo_module()), &RustRules::default());
        assert!(out
            .contains("pub fn decode_create_charge_error(status: u16, body: &str) -> TonoError {"));
        assert!(out.contains(
            "if status == 402 && value.pointer(\"/code\").and_then(|v| v.as_str()) == Some(\"payment_declined\") {"
        ));
        assert!(out.contains("serde_json::from_value::<PaymentDeclined>(value.clone())"));
        assert!(out.contains("return TonoError::Api(APIFailure::PaymentDeclined(data));"));
        assert!(out.contains("if status == 429 {"));
        assert!(out.contains(
            "TonoError::Api(APIFailure::Undeclared(APIError { status, body: body.to_string() }))"
        ));
    }
}

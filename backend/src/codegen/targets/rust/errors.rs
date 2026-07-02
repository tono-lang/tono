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

use crate::codegen::casing::{transform, CasingConfig};
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::ops::{
    declared_errors, discrimination_order, effect_of, module_declared_errors, DeclaredError, Effect,
};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::rust::types::type_expr_of;
use crate::codegen::tree::{ClientDecl, Decl, Field, Method, Raw, TypeExpr};
use crate::ir::{Module, Shape};

/// The canonical taxonomy type names, derived through the same casing engine
/// as every other type identifier (so `api_error` follows the initialism set).
struct Names {
    root: String,
    api: String,
    api_failure: String,
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
        api_failure: type_ident_from_id("api_failure"),
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

fn variant_name(err: &DeclaredError) -> String {
    type_ident_from_id(&err.shape_id)
}

/// The closed error taxonomy plus the module's `Api` payload enum. The
/// category structs are plain data; the `TonoError` enum carries exactly one
/// variant per category and implements `Display`/`Error` (with the transport
/// and contract causes as sources) and the retryable predicate.
pub fn taxonomy_decls(module: &Module) -> Vec<Decl> {
    let n = names();
    let declared = module_declared_errors(module);
    let mut decls = vec![
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub field: String,\n    pub constraint: String,\n    pub message: String,\n}}",
            n.violation
        )),
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub violations: Vec<{}>,\n}}",
            n.validation, n.violation
        )),
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub cause: Box<dyn std::error::Error + Send + Sync>,\n}}",
            n.transport
        )),
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub path: String,\n    pub expected: String,\n    pub raw: String,\n}}",
            n.decode
        )),
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub contract_name: String,\n    pub cause: Box<dyn std::error::Error + Send + Sync>,\n}}",
            n.contract
        )),
        raw(format!(
            "#[derive(Debug)]\npub struct {} {{\n    pub status: u16,\n    pub body: String,\n}}",
            n.api
        )),
    ];

    let mut failure_variants: Vec<String> = declared
        .iter()
        .map(|err| format!("    {}({}),\n", variant_name(err), variant_name(err)))
        .collect();
    failure_variants.push(format!("    Undeclared({}),\n", n.api));
    decls.push(raw(format!(
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
                variant_name(err)
            )
        })
        .collect();
    let failure_retryable_body = if retryable_arms.is_empty() {
        "        false".to_string()
    } else {
        format!("        match self {{\n{retryable_arms}            _ => false,\n        }}")
    };
    decls.push(raw(format!(
        "impl {} {{\n    pub fn retryable(&self) -> bool {{\n{failure_retryable_body}\n    }}\n}}",
        n.api_failure
    )));

    decls.push(raw(format!(
        "#[derive(Debug)]\npub enum {root} {{\n    Validation({validation}),\n    Transport({transport}),\n    Api({api_failure}),\n    Decode({decode}),\n    Contract({contract}),\n}}",
        root = n.root,
        validation = n.validation,
        transport = n.transport,
        api_failure = n.api_failure,
        decode = n.decode,
        contract = n.contract,
    )));

    decls.push(raw(format!(
        "impl {root} {{\n    pub fn retryable(&self) -> bool {{\n        match self {{\n            {root}::Api(failure) => failure.retryable(),\n            _ => false,\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls.push(raw(format!(
        "impl std::fmt::Display for {root} {{\n    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {{\n        match self {{\n            {root}::Validation(_) => write!(f, \"validation failed\"),\n            {root}::Transport(_) => write!(f, \"transport failure\"),\n            {root}::Api(_) => write!(f, \"api error\"),\n            {root}::Decode(_) => write!(f, \"response body did not match the declared schema\"),\n            {root}::Contract(e) => write!(f, \"contract hook '{{}}' failed\", e.contract_name),\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls.push(raw(format!(
        "impl std::error::Error for {root} {{\n    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {{\n        match self {{\n            {root}::Transport(e) => Some(e.cause.as_ref()),\n            {root}::Contract(e) => Some(e.cause.as_ref()),\n            _ => None,\n        }}\n    }}\n}}",
        root = n.root
    )));

    decls
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

/// The client trait: one method per operation. An async operation is an
/// `async fn`; every method returns `Result<_, TonoError>`, the native error
/// idiom.
pub fn client_decl(module: &Module, config: &CasingConfig) -> Decl {
    let n = names();
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
                err: Some(TypeExpr::Ref(Symbol::builtin(n.root.clone()))),
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
/// errors: `(status, raw body) -> TonoError`. The mapping tries the declared
/// errors (coded entries before a codeless catch-all on the same status) and
/// resolves everything else to `Undeclared` carrying the concrete fallback
/// type.
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
    for err in &ordered {
        let data = variant_name(err);
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
    raw(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::target::RenderRules;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::targets::rust::RustRules;
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
            .map(|d| RustRules.render_decl(d))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn the_root_is_an_enum_with_exactly_one_variant_per_category() {
        let out = rendered(&taxonomy_decls(&demo_module()));
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
        let out = rendered(&taxonomy_decls(&demo_module()));
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
        let mut module = demo_module();
        for shape in &mut module.shapes {
            shape.traits.retain(|t| t.id != "retryable");
        }
        let out = rendered(&taxonomy_decls(&module));
        assert!(out.contains("pub fn retryable(&self) -> bool {\n        false\n    }"));
    }

    #[test]
    fn the_client_trait_lowers_the_effect_to_async_fn_returning_result() {
        let out = rendered(&[client_decl(&demo_module(), &rust_casing())]);
        assert!(out.contains("#[allow(async_fn_in_trait)]"));
        assert!(out.contains(
            "    async fn create_charge(&self, input: ChargeInput) -> Result<Charge, TonoError>;"
        ));
    }

    #[test]
    fn a_sync_operation_keeps_a_plain_fn() {
        let mut module = demo_module();
        module.operations = vec![op("m#local_sum", vec![], vec![])];
        let out = rendered(&[client_decl(&module, &rust_casing())]);
        assert!(out
            .contains("    fn local_sum(&self, input: ChargeInput) -> Result<Charge, TonoError>;"));
        assert!(!out.contains("async fn"));
    }

    #[test]
    fn the_discriminator_matches_status_and_code_then_falls_back() {
        let out = rendered(&discriminator_decls(&demo_module()));
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

    #[test]
    fn an_operation_with_no_declared_errors_gets_no_discriminator() {
        let mut module = demo_module();
        module.operations = vec![op("m#ping", vec![], vec![])];
        assert!(discriminator_decls(&module).is_empty());
    }
}

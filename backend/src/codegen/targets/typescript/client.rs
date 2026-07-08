//! The TypeScript HTTP client implementation: a concrete `HttpClient` class
//! whose methods embed the operation's opaque wire descriptor and hand it to the
//! runtime's `execute`, then map the runtime's raw outcome onto the generated
//! error taxonomy. The class is the seam's Target side: it never interprets the
//! descriptor (it serializes it to a JSON string the runtime parses), which is
//! what keeps the Target blind to protocol.
//!
//! The error taxonomy is single-sourced in the generated SDK (a network failure
//! becomes `TransportError`, a 2xx body mismatch becomes `DecodeError`, a non-2xx
//! status goes through the generated discriminator to a declared error or
//! `APIError`), so every language raises errors its own idiomatic way while the
//! runtime stays a thin, error-class-free transport.

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::ops::{declared_errors, error_names, method_ident, op_io, wire_descriptor};
use crate::codegen::symbol::Symbol;
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape, Tref};

/// The runtime package the generated TypeScript SDK depends on for transport.
pub const RUNTIME_PKG: &str = "@tono/http-runtime-ts";

/// The local name of a shape reference, or `None` for a non-reference type. Used
/// to pick the generated `encode`/`decode` helper for an operation's input and
/// output.
fn ref_name(t: &Tref) -> Option<String> {
    match t {
        Tref::Ref { id, .. } => Some(type_ident_from_id(id)),
        _ => None,
    }
}

/// A self-module type symbol, imported from the types file via the serde file's
/// companion redirect (mirrors `errors.rs`).
fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// The JSON descriptor embedded as a JavaScript string literal the runtime
/// parses at load. Encoding to text twice (object -> JSON, JSON -> JS string
/// literal) treats the descriptor as an opaque blob: no field is ever read.
fn embed(descriptor: &serde_json::Value) -> String {
    let json = serde_json::to_string(descriptor).unwrap_or_else(|_| "null".into());
    serde_json::to_string(&json).unwrap_or_else(|_| "\"null\"".into())
}

/// One operation's method body plus the module type symbols it references (for
/// import collection). The method embeds the descriptor, calls `execute`, and
/// maps the outcome onto the taxonomy.
fn op_method(
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    refs: &mut Vec<Symbol>,
) -> String {
    let n = error_names();
    let name = method_ident(op, config, LANG);
    let (input, output) = op_io(op);

    let (param, input_expr) = match input {
        Some(t) => {
            let ty = render_type(&type_expr_of(t), &TsRules);
            let expr = match ref_name(t) {
                Some(name) => format!("encode{name}(input)"),
                None => "input".to_string(),
            };
            (format!("input: {ty}"), expr)
        }
        None => (String::new(), "{}".to_string()),
    };

    let ret = output
        .map(|t| render_type(&type_expr_of(t), &TsRules))
        .unwrap_or_else(|| "void".to_string());

    let desc_const = format!("{name}Descriptor");

    // Non-2xx mapping: the generated discriminator when the op declares errors,
    // otherwise the generic fallback.
    let error_line = if declared_errors(op, module).is_empty() {
        refs.push(module_symbol(&n.api, module));
        format!("throw new {}(outcome.status, outcome.body);", n.api)
    } else {
        format!(
            "throw decode{}Error(outcome.status, outcome.body);",
            type_ident_from_id(&op.id)
        )
    };

    // 2xx mapping: decode the body to the output type, or return nothing when the
    // operation has no output. A decode failure is a schema mismatch.
    let success_block = match output.and_then(ref_name) {
        Some(out_name) => {
            refs.push(module_symbol(&n.decode, module));
            format!(
                "    try {{\n      return decode{out}(JSON.parse(outcome.body));\n    }} catch (cause) {{\n      throw new {decode}(\"$\", \"{out}\", outcome.body);\n    }}",
                out = out_name,
                decode = n.decode
            )
        }
        None if output.is_some() => {
            format!("    return JSON.parse(outcome.body) as {ret};")
        }
        None => "    return;".to_string(),
    };

    refs.push(module_symbol(&n.transport, module));

    format!(
        "  async {name}({param}): Promise<{ret}> {{\n\
         \x20   const outcome = await execute({desc_const}, {input_expr}, this.options);\n\
         \x20   if (outcome.outcome === \"transport\") {{\n\
         \x20     throw new {transport}(outcome.cause);\n\
         \x20   }}\n\
         \x20   if (outcome.outcome === \"error\") {{\n\
         \x20     {error_line}\n\
         \x20   }}\n\
         {success_block}\n\
         \x20 }}",
        transport = n.transport
    )
}

/// The `HttpClient` class for a module: one async method per HTTP operation,
/// preceded by the per-operation descriptor constants. Returns nothing when the
/// module has no HTTP operation (a purely local module needs no transport
/// client).
pub fn client_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let http_ops: Vec<(&Shape, &serde_json::Value)> = module
        .operations
        .iter()
        .filter_map(|op| wire_descriptor(op).map(|d| (op, d)))
        .collect();
    if http_ops.is_empty() {
        return Vec::new();
    }

    let mut refs = vec![
        Symbol::imported("execute", RUNTIME_PKG, "execute"),
        Symbol::imported("WireDescriptor", RUNTIME_PKG, "WireDescriptor"),
        Symbol::imported("ClientOptions", RUNTIME_PKG, "ClientOptions"),
    ];

    let mut consts = String::new();
    let mut methods = String::new();
    for (op, descriptor) in &http_ops {
        let name = method_ident(op, config, LANG);
        consts.push_str(&format!(
            "const {name}Descriptor: WireDescriptor = JSON.parse({});\n",
            embed(descriptor)
        ));
        methods.push_str(&op_method(op, module, config, &mut refs));
        methods.push('\n');
    }

    // Only claim to implement the client interface when every operation is an
    // HTTP one; a module mixing local operations would leave methods unimplemented.
    let all_http = module.operations.len() == http_ops.len();
    let implements = if all_http {
        let client = type_ident_from_id("client");
        refs.push(module_symbol(&client, module));
        format!(" implements {client}")
    } else {
        String::new()
    };

    let class = format!(
        "{consts}export class HttpClient{implements} {{\n  constructor(private readonly options: ClientOptions) {{}}\n\n{methods}}}"
    );
    vec![Decl::raw_with(class, refs)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::{member, operation, rendered, structure};
    use crate::ir::{Module, Prim, Trait, Tref};
    use serde_json::json;

    fn descriptor_trait(value: serde_json::Value) -> Trait {
        Trait {
            id: "wire_descriptor".into(),
            value,
        }
    }

    fn http_module(descriptor: serde_json::Value) -> Module {
        Module {
            name: "m".into(),
            shapes: vec![
                structure(
                    "m#charge_input",
                    vec![member("amount", Tref::Prim(Prim::String), true)],
                ),
                structure(
                    "m#charge",
                    vec![member("id", Tref::Prim(Prim::String), true)],
                ),
            ],
            operations: vec![operation(
                "m#create_charge",
                vec![descriptor_trait(descriptor)],
                vec![],
            )],
        }
    }

    fn client_text(module: &Module) -> String {
        rendered(&client_decls(module, &ts_casing()), &TsRules)
    }

    #[test]
    fn the_client_implements_the_interface_and_calls_execute() {
        let out = client_text(&http_module(json!({"http_method": "POST", "uri": "/charges"})));
        assert!(out.contains("export class HttpClient implements Client {"));
        assert!(out.contains("async createCharge(input: ChargeInput): Promise<Charge> {"));
        assert!(out.contains("await execute(createChargeDescriptor, encodeChargeInput(input), this.options);"));
        // A network failure and a schema mismatch raise the SDK's own taxonomy;
        // the runtime raises nothing itself.
        assert!(out.contains("throw new TransportError(outcome.cause);"));
        assert!(out.contains("return decodeCharge(JSON.parse(outcome.body));"));
        assert!(out.contains("throw new DecodeError(\"$\", \"Charge\", outcome.body);"));
    }

    #[test]
    fn the_descriptor_is_embedded_verbatim_as_an_opaque_blob() {
        // An unknown field the Target cannot understand rides through unchanged:
        // proof the descriptor is embedded, never interpreted. The embedded form is
        // the descriptor serialized to a JSON string the runtime parses at load.
        let descriptor = json!({
            "http_method": "POST",
            "uri": "/charges",
            "mystery_field": [1, 2, 3],
        });
        let out = client_text(&http_module(descriptor.clone()));
        assert!(out.contains(&format!(
            "const createChargeDescriptor: WireDescriptor = JSON.parse({});",
            embed(&descriptor)
        )));
        assert!(out.contains("mystery_field"));
    }

    #[test]
    fn an_operation_with_no_declared_errors_falls_back_to_the_api_error() {
        let out = client_text(&http_module(json!({"http_method": "GET", "uri": "/x"})));
        assert!(out.contains("throw new APIError(outcome.status, outcome.body);"));
    }

    #[test]
    fn a_module_with_no_http_operation_emits_no_client() {
        // An operation without a wire descriptor is local: no transport client.
        let module = Module {
            name: "m".into(),
            shapes: vec![],
            operations: vec![operation("m#local", vec![], vec![])],
        };
        assert!(client_decls(&module, &ts_casing()).is_empty());
    }
}

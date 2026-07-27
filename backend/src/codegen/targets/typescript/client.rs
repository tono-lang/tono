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

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::conventions::type_ident_from_id;
use crate::codegen::extensions::{bound_extensions, hook_binding, BoundExtension};
use crate::codegen::ops::{declared_errors, error_names, method_ident, op_io, wire_descriptor};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::{type_expr_of, LANG};
use crate::codegen::tree::Decl;
use crate::ir::{ExtKind, Module, Shape, Tref};

/// The runtime package the generated TypeScript SDK depends on for transport.
pub const RUNTIME_PKG: &str = "@tono/http-runtime-ts";

/// The binding-language keys the TypeScript target reads from the extension
/// table (the short form and its full name).
const BINDING_LANGS: [&str; 2] = ["ts", "typescript"];

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

/// Which lifecycle slots the module binds for TypeScript. Drives whether the
/// client passes hooks into `execute`, applies `client_init` at construction,
/// and routes thrown errors through `on_error`.
struct Lifecycle {
    client_init: bool,
    before_request: bool,
    after_response: bool,
    on_error: bool,
}

impl Lifecycle {
    fn of(bound: &[BoundExtension<'_>]) -> Self {
        let has = |slot| hook_binding(bound, slot).is_some();
        Self {
            client_init: has("client_init"),
            before_request: has("before_request"),
            after_response: has("after_response"),
            on_error: has("on_error"),
        }
    }

    /// The runtime is handed hooks only for the slots it invokes around the
    /// transport (`before_request`/`after_response`).
    fn passes_hooks(&self) -> bool {
        self.before_request || self.after_response
    }
}

/// One boundary wrapper: call the bound symbol, let a declared error pass typed,
/// and turn anything else into a `ContractError` naming the slot. The
/// pass-through-vs-wrap choice is the load-bearing invariant of the model, so it
/// is spelled out in the emitted code.
fn wrapper(slot: &str, root: &str, contract: &str, signature: &str, call: &str) -> String {
    format!(
        "function {fname}{signature} {{\n\
         \x20 try {{\n\
         \x20   return {call};\n\
         \x20 }} catch (e) {{\n\
         \x20   // A declared SDK error passes through typed; anything else is bespoke\n\
         \x20   // failure surfaced as a ContractError.\n\
         \x20   if (e instanceof {root}) throw e;\n\
         \x20   throw new {contract}(\"{slot}\", e);\n\
         \x20 }}\n\
         }}",
        fname = wrapper_name(slot),
    )
}

/// A bound file path (`ext/ts/auth.ts`) as a TypeScript import specifier. The
/// extension is dropped (`./ext/ts/auth`), matching the SDK's own extensionless
/// imports and staying valid where a `.ts` specifier is not (NodeNext/ESM). The
/// path is relative to the generated file, so it gets an explicit `./` unless the
/// user already wrote one; the renderer keeps a dot-prefixed specifier verbatim
/// rather than reading its dots as module segments.
pub(crate) fn import_specifier(module: &str) -> String {
    let path = module
        .strip_suffix(".ts")
        .or_else(|| module.strip_suffix(".tsx"))
        .unwrap_or(module);
    if path.starts_with('.') || path.starts_with('/') {
        path.to_string()
    } else {
        format!("./{path}")
    }
}

/// The generated name of a slot's boundary wrapper.
fn wrapper_name(slot: &str) -> String {
    match slot {
        "client_init" => "wrapClientInit",
        "before_request" => "wrapBeforeRequest",
        "after_response" => "wrapAfterResponse",
        "on_error" => "wrapOnError",
        _ => "wrapHook",
    }
    .to_string()
}

/// Emit every bound-hook wrapper, importing each bound symbol and the runtime
/// types the wrapper signatures need.
fn hook_wrappers(bound: &[BoundExtension<'_>], module: &Module, refs: &mut Vec<Symbol>) -> String {
    let n = error_names();
    let mut out = String::new();
    // Every wrapper's boundary catch references the taxonomy root and the
    // ContractError, so pull them in once rather than per hook.
    let any_hook = crate::codegen::extensions::HOOK_SLOTS
        .iter()
        .any(|slot| hook_binding(bound, slot).is_some());
    if any_hook {
        refs.push(module_symbol(&n.root, module));
        refs.push(module_symbol(&n.contract, module));
    }
    for slot in crate::codegen::extensions::HOOK_SLOTS {
        let Some(binding) = hook_binding(bound, slot) else {
            continue;
        };
        refs.push(Symbol::imported(
            binding.symbol,
            import_specifier(binding.module),
            binding.symbol,
        ));
        let (signature, call) = match slot {
            "client_init" => {
                refs.push(Symbol::imported(
                    "ClientOptions",
                    RUNTIME_PKG,
                    "ClientOptions",
                ));
                (
                    "(): Partial<ClientOptions>".to_string(),
                    format!("{}()", binding.symbol),
                )
            }
            "before_request" => {
                refs.push(Symbol::imported(
                    "CanonicalRequest",
                    RUNTIME_PKG,
                    "CanonicalRequest",
                ));
                (
                    "(req: CanonicalRequest): Promise<CanonicalRequest>".to_string(),
                    format!("await {}(req)", binding.symbol),
                )
            }
            "after_response" => {
                refs.push(Symbol::imported(
                    "CanonicalResponse",
                    RUNTIME_PKG,
                    "CanonicalResponse",
                ));
                (
                    "(res: CanonicalResponse): Promise<CanonicalResponse>".to_string(),
                    format!("await {}(res)", binding.symbol),
                )
            }
            _ => (
                format!("(err: {root}): {root}", root = n.root),
                format!("{}(err)", binding.symbol),
            ),
        };
        // before_request/after_response are async (they await the bound symbol).
        let async_kw = if matches!(slot, "before_request" | "after_response") {
            "async "
        } else {
            ""
        };
        out.push_str(async_kw);
        out.push_str(&wrapper(slot, &n.root, &n.contract, &signature, &call));
        out.push_str("\n\n");
    }
    out
}

/// The generated name of a contract/constraint boundary wrapper: `wrapped` plus
/// the PascalCase extension name (`sign_request` -> `wrappedSignRequest`).
fn contract_wrapper_name(name: &str) -> String {
    format!(
        "wrapped{}",
        transform(
            name,
            SymbolKind::Type,
            &CasingConfig::new(CaseStyle::Pascal),
            None
        )
    )
}

/// Emit a boundary wrapper for each bound contract/constraint that declares a
/// signature, mirroring the Rust/Go glue: it calls the bound symbol and turns any
/// non-declared failure into a `ContractError` while a declared error passes
/// through. Hooks are handled by `hook_wrappers` (their fixed slots carry
/// runtime-typed signatures); a contract/constraint carries a user-typed one, so
/// without this a ts-bound contract would generate no code and its binding would
/// be silently unused.
pub(crate) fn contract_wrappers(
    bound: &[BoundExtension<'_>],
    module: &Module,
    refs: &mut Vec<Symbol>,
) -> String {
    let n = error_names();
    let contracts: Vec<&BoundExtension<'_>> = bound
        .iter()
        .filter(|e| e.kind != ExtKind::Hook && e.signature.is_some())
        .collect();
    if contracts.is_empty() {
        return String::new();
    }
    // The boundary catch references the taxonomy root and the ContractError, so
    // pull them in once rather than per wrapper.
    refs.push(module_symbol(&n.root, module));
    refs.push(module_symbol(&n.contract, module));
    let mut out = String::new();
    for e in contracts {
        let sig = e.signature.expect("filtered to Some");
        refs.push(Symbol::imported(
            e.symbol,
            import_specifier(e.module),
            e.symbol,
        ));
        let input = render_type(&type_expr_of(&sig.input), &TsRules);
        let output = render_type(&type_expr_of(&sig.output), &TsRules);
        out.push_str(&format!(
            "function {fname}(input: {input}): {output} {{\n\
             \x20 try {{\n\
             \x20   return {sym}(input);\n\
             \x20 }} catch (e) {{\n\
             \x20   // A declared SDK error passes through typed; anything else is bespoke\n\
             \x20   // failure surfaced as a ContractError.\n\
             \x20   if (e instanceof {root}) throw e;\n\
             \x20   throw new {contract}(\"{name}\", e);\n\
             \x20 }}\n\
             }}\n\n",
            fname = contract_wrapper_name(e.name),
            sym = e.symbol,
            root = n.root,
            contract = n.contract,
            name = e.name,
        ));
    }
    out
}

/// One operation's method body plus the module type symbols it references (for
/// import collection). The method embeds the descriptor, calls `execute`, and
/// maps the outcome onto the taxonomy, routing each thrown error through
/// `on_error` when that hook is bound.
fn op_method(
    op: &Shape,
    module: &Module,
    config: &CasingConfig,
    life: &Lifecycle,
    refs: &mut Vec<Symbol>,
) -> String {
    let n = error_names();
    let name = method_ident(op, config, LANG);
    let (input, output) = op_io(op);

    // A bound `on_error` intercepts every thrown error before it leaves the SDK.
    let throw = |expr: String| {
        if life.on_error {
            format!("throw {}({expr});", wrapper_name("on_error"))
        } else {
            format!("throw {expr};")
        }
    };

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

    // An input whose shape carries lowerable constraints is rejected before it
    // reaches the transport: the generated validator runs first and its
    // ValidationError is raised like every other taxonomy error (through
    // `on_error` when bound).
    let validate_stmt = match input {
        Some(Tref::Ref { id, .. }) => module
            .shapes
            .iter()
            .find(|s| s.id == *id)
            .filter(|s| crate::codegen::validation::shape_has_checks(s))
            .map(|_| {
                let input_name = type_ident_from_id(id);
                refs.push(module_symbol(&format!("validate{input_name}"), module));
                format!(
                    "\x20   const invalid = validate{input_name}(input);\n\x20   if (invalid) {{\n\x20     {}\n\x20   }}\n",
                    throw("invalid".to_string())
                )
            })
            .unwrap_or_default(),
        _ => String::new(),
    };

    let ret = output
        .map(|t| render_type(&type_expr_of(t), &TsRules))
        .unwrap_or_else(|| "void".to_string());

    let desc_const = format!("{name}Descriptor");

    // Non-2xx mapping: the generated discriminator when the op declares errors,
    // otherwise the generic fallback.
    let error_line = if declared_errors(op, module).is_empty() {
        refs.push(module_symbol(&n.api, module));
        throw(format!("new {}(outcome.status, outcome.body)", n.api))
    } else {
        throw(format!(
            "decode{}Error(outcome.status, outcome.body)",
            type_ident_from_id(&op.id)
        ))
    };

    // 2xx mapping: decode the body to the output type, or return nothing when the
    // operation has no output. A decode failure is a schema mismatch.
    let success_block = match output.and_then(ref_name) {
        Some(out_name) => {
            refs.push(module_symbol(&n.decode, module));
            format!(
                "    try {{\n      return decode{out}(JSON.parse(outcome.body));\n    }} catch {{\n      throw new {decode}(\"$\", \"{out}\", outcome.body);\n    }}",
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
    let hooks_arg = if life.passes_hooks() {
        ", this.hooks"
    } else {
        ""
    };
    let transport_throw = throw(format!("new {}(outcome.cause)", n.transport));

    format!(
        "  async {name}({param}): Promise<{ret}> {{\n\
         {validate_stmt}\
         \x20   const outcome = await execute({desc_const}, {input_expr}, this.options{hooks_arg});\n\
         \x20   if (outcome.outcome === \"transport\") {{\n\
         \x20     {transport_throw}\n\
         \x20   }}\n\
         \x20   if (outcome.outcome === \"error\") {{\n\
         \x20     {error_line}\n\
         \x20   }}\n\
         {success_block}\n\
         \x20 }}"
    )
}

/// The class preamble: the `hooks` field (when the runtime is handed any) and the
/// constructor (which applies `client_init` when bound).
fn class_preamble(life: &Lifecycle, refs: &mut Vec<Symbol>) -> String {
    let mut out = String::new();
    if life.passes_hooks() {
        refs.push(Symbol::imported("Hooks", RUNTIME_PKG, "Hooks"));
        let mut slots = Vec::new();
        if life.before_request {
            slots.push(format!(
                "before_request: {}",
                wrapper_name("before_request")
            ));
        }
        if life.after_response {
            slots.push(format!(
                "after_response: {}",
                wrapper_name("after_response")
            ));
        }
        out.push_str(&format!(
            "  private readonly hooks: Hooks = {{ {} }};\n",
            slots.join(", ")
        ));
    }
    if life.client_init {
        out.push_str(&format!(
            "  private readonly options: ClientOptions;\n  constructor(options: ClientOptions) {{\n    this.options = {{ ...options, ...{}() }};\n  }}\n",
            wrapper_name("client_init")
        ));
    } else {
        out.push_str("  constructor(private readonly options: ClientOptions) {}\n");
    }
    out
}

/// The `HttpClient` class for a module: one async method per HTTP operation,
/// preceded by the per-operation descriptor constants and any bound-hook and
/// bound-contract/constraint wrappers. Returns nothing when the module has no HTTP
/// operation (a purely local module needs no transport client, and the wrappers
/// ride the operation surface that gives them a ContractError to wrap into).
pub fn client_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let http_ops: Vec<(&Shape, &serde_json::Value)> = module
        .operations
        .iter()
        .filter_map(|op| wire_descriptor(op).map(|d| (op, d)))
        .collect();
    if http_ops.is_empty() {
        return Vec::new();
    }

    let bound = bound_extensions(module, &BINDING_LANGS);
    let life = Lifecycle::of(&bound);

    let mut refs = vec![
        Symbol::imported("execute", RUNTIME_PKG, "execute"),
        Symbol::imported("WireDescriptor", RUNTIME_PKG, "WireDescriptor"),
        Symbol::imported("ClientOptions", RUNTIME_PKG, "ClientOptions"),
    ];

    let mut wrappers = hook_wrappers(&bound, module, &mut refs);
    wrappers.push_str(&contract_wrappers(&bound, module, &mut refs));

    let mut consts = String::new();
    let mut methods = String::new();
    for (op, descriptor) in &http_ops {
        let name = method_ident(op, config, LANG);
        consts.push_str(&format!(
            "const {name}Descriptor: WireDescriptor = JSON.parse({});\n",
            embed(descriptor)
        ));
        methods.push_str(&op_method(op, module, config, &life, &mut refs));
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

    let preamble = class_preamble(&life, &mut refs);
    let class = format!(
        "{consts}{wrappers}export class HttpClient{implements} {{\n{preamble}\n{methods}}}"
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
            extensions: vec![],
        }
    }

    fn client_text(module: &Module) -> String {
        rendered(&client_decls(module, &ts_casing()), &TsRules)
    }

    #[test]
    fn the_client_implements_the_interface_and_calls_execute() {
        let out = client_text(&http_module(
            json!({"http_method": "POST", "uri": "/charges"}),
        ));
        assert!(out.contains("export class HttpClient implements Client {"));
        assert!(out.contains("async createCharge(input: ChargeInput): Promise<Charge> {"));
        assert!(out.contains(
            "await execute(createChargeDescriptor, encodeChargeInput(input), this.options);"
        ));
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
    fn a_constrained_input_is_validated_before_the_transport() {
        use crate::codegen::test_support::member_constrained;
        let mut module = http_module(json!({"http_method": "POST", "uri": "/charges"}));
        module.shapes[0] = structure(
            "m#charge_input",
            vec![member_constrained(
                "amount",
                Tref::Prim(Prim::I64),
                vec![crate::ir::Constraint::Range {
                    min: Some(0.0),
                    max: None,
                    excl_min: false,
                    excl_max: false,
                }],
            )],
        );
        let out = client_text(&module);
        // The generated validator runs first and its ValidationError is raised
        // like every other taxonomy error, before anything reaches the transport.
        assert!(out.contains("const invalid = validateChargeInput(input);"));
        assert!(out.contains("throw invalid;"));
        let validate_at = out
            .find("const invalid = validateChargeInput(input);")
            .expect("validate call");
        let execute_at = out
            .find("await execute(createChargeDescriptor")
            .expect("execute call");
        assert!(
            validate_at < execute_at,
            "validation must precede the transport call"
        );
    }

    #[test]
    fn an_unconstrained_input_skips_validation() {
        let out = client_text(&http_module(
            json!({"http_method": "POST", "uri": "/charges"}),
        ));
        assert!(!out.contains("validateChargeInput"));
    }

    #[test]
    fn a_bound_on_error_routes_the_validation_rejection() {
        use crate::codegen::test_support::member_constrained;
        let mut module = http_module(json!({"http_method": "POST", "uri": "/charges"}));
        module.shapes[0] = structure(
            "m#charge_input",
            vec![member_constrained(
                "amount",
                Tref::Prim(Prim::I64),
                vec![crate::ir::Constraint::Range {
                    min: Some(0.0),
                    max: None,
                    excl_min: false,
                    excl_max: false,
                }],
            )],
        );
        module.extensions = vec![hook("on_error", "ext/ts/errors.ts#mapError")];
        let out = client_text(&module);
        assert!(out.contains("throw wrapOnError(invalid);"));
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
            extensions: vec![],
        };
        assert!(client_decls(&module, &ts_casing()).is_empty());
    }

    fn hook(name: &str, target: &str) -> crate::ir::Extension {
        crate::ir::Extension {
            name: name.into(),
            kind: crate::ir::ExtKind::Hook,
            signature: None,
            raw: false,
            bindings: [("ts".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: None,
        }
    }

    #[test]
    fn a_bound_before_request_wraps_the_boundary_and_feeds_the_runtime() {
        let mut module = http_module(json!({"http_method": "POST", "uri": "/charges"}));
        module.extensions = vec![hook("before_request", "ext/ts/auth.ts#addBearer")];
        let out = client_text(&module);

        // The wrapper spells out the pass-through-vs-wrap decision in the emitted code.
        assert!(out.contains(
            "async function wrapBeforeRequest(req: CanonicalRequest): Promise<CanonicalRequest> {"
        ));
        assert!(out.contains("return await addBearer(req);"));
        assert!(out.contains("if (e instanceof TonoError) throw e;"));
        assert!(out.contains("throw new ContractError(\"before_request\", e);"));
        // The runtime is handed the hook and the method passes it into execute.
        assert!(
            out.contains("private readonly hooks: Hooks = { before_request: wrapBeforeRequest };")
        );
        assert!(out.contains(
            "await execute(createChargeDescriptor, encodeChargeInput(input), this.options, this.hooks);"
        ));
    }

    #[test]
    fn a_bound_hook_is_imported_and_wired_through_the_generated_serde_file() {
        // End-to-end through the pipeline: the bound symbol is imported from its
        // relative file (extensionless, not read as module segments) and the
        // runtime is handed the hooks object.
        use crate::codegen::modules::CodegenConfig;
        use crate::codegen::pipeline::generate;
        use crate::codegen::TargetKind;

        let mut module = http_module(json!({"http_method": "GET", "uri": "/charges"}));
        module.extensions = vec![hook("before_request", "ext/ts/auth.ts#addBearer")];
        let model = crate::ir::Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![module],
        };
        let files = generate(&model, &[TargetKind::TypeScript], &CodegenConfig::default()).unwrap();
        let serde = files
            .iter()
            .find(|f| f.path.to_string_lossy().replace('\\', "/") == "typescript/m_serde.ts")
            .unwrap();
        assert!(serde
            .text
            .contains("import { addBearer } from \"./ext/ts/auth\";"));
        assert!(serde
            .text
            .contains("private readonly hooks: Hooks = { before_request: wrapBeforeRequest };"));
        assert!(serde.text.contains("this.options, this.hooks);"));
    }

    #[test]
    fn a_bound_client_init_and_on_error_wire_construction_and_the_error_path() {
        let mut module = http_module(json!({"http_method": "GET", "uri": "/x"}));
        module.extensions = vec![
            hook("client_init", "ext/ts/settings.ts#initSettings"),
            hook("on_error", "ext/ts/errors.ts#mapError"),
        ];
        let out = client_text(&module);

        // client_init merges Settings into the options at construction.
        assert!(out.contains("function wrapClientInit(): Partial<ClientOptions> {"));
        assert!(out.contains("this.options = { ...options, ...wrapClientInit() };"));
        // on_error intercepts each thrown error before it leaves the SDK.
        assert!(out.contains("function wrapOnError(err: TonoError): TonoError {"));
        assert!(out.contains("throw wrapOnError(new TransportError(outcome.cause));"));
        assert!(out.contains("throw wrapOnError(new APIError(outcome.status, outcome.body));"));
        // client_init is not a transport hook, so no hooks object is passed.
        assert!(out.contains(", this.options);"));
    }

    fn contract(name: &str, target: &str) -> crate::ir::Extension {
        crate::ir::Extension {
            name: name.into(),
            kind: crate::ir::ExtKind::Contract,
            signature: Some(crate::ir::Signature {
                input: Tref::Prim(Prim::String),
                output: Tref::Prim(Prim::String),
            }),
            raw: false,
            bindings: [("ts".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: Some("v.json".into()),
        }
    }

    #[test]
    fn a_bound_contract_emits_a_ts_boundary_wrapper() {
        // A ts-bound contract emits a boundary wrapper (parity with Rust/Go), so
        // the binding is not silently unused.
        let mut module = http_module(json!({"http_method": "GET", "uri": "/x"}));
        module.extensions = vec![contract("sign_request", "ext/ts/sign.ts#signRequest")];
        let out = client_text(&module);

        // Imports are collected at file-render level (covered by the serde-file
        // test); here we assert the wrapper body the client emits.
        assert!(out.contains("function wrappedSignRequest(input: string): string {"));
        assert!(out.contains("return signRequest(input);"));
        // The pass-through-vs-wrap idiom, same as the hook wrappers.
        assert!(out.contains("if (e instanceof TonoError) throw e;"));
        assert!(out.contains("throw new ContractError(\"sign_request\", e);"));
    }
}

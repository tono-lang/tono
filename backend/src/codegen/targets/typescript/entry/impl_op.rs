//! The TypeScript glue for an operation implemented by bespoke sources
//! (`ext impl`).
//!
//! Two forms, one error boundary. In the typed form the bound symbol speaks the
//! operation's own types and the glue only guards the boundary: a declared SDK
//! error passes through typed (with its `retryable()` and everything), anything
//! else becomes a `ContractError` naming the operation. In the raw form the
//! bound symbol returns an outcome and the glue integrates it declaratively,
//! exactly as it integrates a protocol response: decode the success payload
//! strictly into the declared output, or discriminate the failure by its code
//! against the operation's declared error codes.

use crate::codegen::entries::op_local_name;
use crate::codegen::extensions::BoundExtension;
use crate::codegen::ops::{declared_errors, error_names};
use crate::codegen::symbol::Symbol;
use crate::ir::{Module, Shape, Tref};

use super::decode::success_block;
use super::module_symbol;
use crate::codegen::targets::typescript::client::import_specifier;

/// Everything the glue needs that the caller already computed: the method
/// surface, the encoded input expression, the boundary error router, and the
/// impl binding this target provides.
pub(super) struct Method<'a> {
    pub op: &'a Shape,
    pub module: &'a Module,
    pub name: &'a str,
    pub param: &'a str,
    pub ret: &'a str,
    pub input_expr: &'a str,
    pub output: Option<&'a Tref>,
    pub validate_block: &'a str,
    pub binding: Option<&'a BoundExtension<'a>>,
    pub throw: &'a dyn Fn(String) -> String,
    pub discriminator: &'a str,
    pub refs: &'a mut Vec<Symbol>,
}

pub(super) fn method(m: Method<'_>) -> String {
    let Method {
        op,
        module,
        name,
        param,
        ret,
        input_expr,
        output,
        validate_block,
        binding,
        throw,
        discriminator,
        refs,
    } = m;
    let en = error_names();
    let local = op_local_name(&op.id);
    refs.push(module_symbol(&en.contract, module));

    let Some(binding) = binding else {
        // Unreachable through the CLI: the emit gate refuses a model whose
        // operation has neither a protocol binding nor an impl for this target.
        // Emitting a method that fails loudly beats emitting one that does not
        // type-check, so a direct library caller that skipped the gate still
        // gets a diagnosable SDK.
        return format!(
            "  async {name}({param}): Promise<{ret}> {{\n    {t}\n  }}",
            t = throw(format!(
                "new {}({local:?}, new Error(\"operation has no implementation for TypeScript\"))",
                en.contract,
            )),
        );
    };
    refs.push(Symbol::imported(
        binding.symbol,
        import_specifier(binding.module),
        binding.symbol,
    ));
    refs.push(module_symbol(&en.root, module));

    // A declared SDK error crosses the boundary untouched; anything else is a
    // bespoke failure the caller cannot be expected to type, so it is named.
    let rethrow = throw("e".to_string());
    let wrapped = throw(format!("new {}({local:?}, e)", en.contract));
    let guard = format!(
        "    }} catch (e) {{\n      if (e instanceof {root}) {rethrow}\n      {wrapped}\n    }}\n",
        root = en.root,
    );

    let body = if binding.raw {
        let failure = if declared_errors(op, module).is_empty() {
            // Status 0: a bespoke outcome carries no protocol status, and the
            // body is what the implementation reported.
            refs.push(module_symbol(&en.api, module));
            throw(format!("new {}(0, outcome.body)", en.api))
        } else {
            throw(format!("{discriminator}(outcome.code, outcome.body)"))
        };
        let success = success_block(output, module, ret, throw, refs);
        format!(
            "    let outcome;\n\
             \x20   try {{\n\
             \x20     outcome = await {sym}(this.settings, JSON.stringify({input_expr}));\n\
             {guard}\
             \x20   if (!outcome.success) {{\n      {failure}\n    }}\n\
             {success}",
            sym = binding.symbol,
        )
    } else {
        let call_args = if param.is_empty() {
            "this.settings".to_string()
        } else {
            "this.settings, input".to_string()
        };
        let tail = if output.is_some() {
            format!(
                "      return await {sym}({call_args});\n",
                sym = binding.symbol
            )
        } else {
            format!(
                "      await {sym}({call_args});\n      return;\n",
                sym = binding.symbol
            )
        };
        format!("    try {{\n{tail}{guard}")
    };

    let signature = bound_signature(binding, param, ret, output.is_some());
    format!(
        "  // {name} is implemented by the bespoke {sym}, imported from\n\
         \x20 // {module_path}:\n\
         \x20 //\n\
         \x20 //   {signature}\n\
         \x20 async {name}({param}): Promise<{ret}> {{\n{validate_block}{body}  }}",
        sym = binding.symbol,
        module_path = binding.module,
    )
}

/// The TypeScript signature the bespoke symbol must have, rendered for the
/// comment above the generated method so the binding is self-documenting.
fn bound_signature(
    binding: &BoundExtension<'_>,
    param: &str,
    ret: &str,
    has_output: bool,
) -> String {
    let settings = "settings: Settings";
    if binding.raw {
        return format!(
            "function {}({settings}, payload: string): Promise<Outcome>",
            binding.symbol
        );
    }
    let params = if param.is_empty() {
        settings.to_string()
    } else {
        format!("{settings}, {param}")
    };
    let out = if has_output { ret } else { "void" };
    format!("function {}({params}): Promise<{out}>", binding.symbol)
}

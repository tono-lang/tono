//! The Go glue for an operation implemented by bespoke sources (`ext impl`).
//!
//! Two forms, one error boundary. In the typed form the bound symbol speaks the
//! operation's own types and the glue only guards the boundary: a declared SDK
//! error passes through typed (with its `Retryable()` and everything), anything
//! else becomes a `ContractError` naming the operation. In the raw form the
//! bound symbol returns an outcome and the glue integrates it declaratively,
//! exactly as it integrates a protocol response: decode the success payload
//! strictly into the declared output, or discriminate the failure by its code
//! against the operation's declared error codes.
//!
//! The bound symbol lives in the generated package: Go's last path segment is
//! often not an importable package name, so the binding path is a "drop this
//! file here" instruction, and the call is unqualified. This mirrors how bound
//! contracts are called.

use crate::codegen::conventions::doc_of;
use crate::codegen::entries::op_local_name;
use crate::codegen::extensions::BoundExtension;
use crate::codegen::ops::{declared_errors, error_names, op_io};
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape};

use super::decode::{success_block, Payload};
use super::{go_type, import, Names};

/// Everything the glue needs that the caller already computed: the method
/// signature and its early-return spellings, the boundary error router, and the
/// impl binding this target provides.
pub(super) struct Method<'a> {
    pub n: &'a Names,
    pub op: &'a Shape,
    pub module: &'a Module,
    pub sig: &'a str,
    pub refs: Vec<Symbol>,
    pub binding: Option<&'a BoundExtension<'a>>,
    pub zero_decl: &'a str,
    pub ret_zero: &'a str,
    pub validate_block: &'a str,
    pub fail: &'a dyn Fn(String) -> String,
    pub discriminator: &'a str,
}

pub(super) fn method_decl(m: Method<'_>) -> Decl {
    let Method {
        n,
        op,
        module,
        sig,
        mut refs,
        binding,
        zero_decl,
        ret_zero,
        validate_block,
        fail,
        discriminator,
    } = m;
    let en = error_names();
    let local = op_local_name(&op.id);
    let (input, output) = op_io(op);
    refs.push(import("errors", "errors"));
    refs.push(import("context", "context"));

    let Some(binding) = binding else {
        // Unreachable through the CLI: the emit gate refuses a model whose
        // operation has neither a protocol binding nor an impl for this target.
        // Emitting a method that fails loudly beats emitting one that does not
        // compile, so a direct library caller that skipped the gate still gets
        // a diagnosable SDK.
        let err = fail(format!(
            "&{contract}{{ContractName: {local:?}, Cause: errors.New(\"operation has no implementation for Go\")}}",
            contract = en.contract,
        ));
        return Decl::raw_with(
            format!(
                "func (c *{client}) {sig} {{\n{zero_decl}\treturn {ret_zero}{err}\n}}",
                client = n.client,
            ),
            refs,
        );
    };

    // A declared SDK error crosses the boundary untouched; anything else is a
    // bespoke failure the caller cannot be expected to type, so it is named.
    let boundary = format!(
        "\tif err != nil {{\n\
         \t\tvar known {marker}\n\
         \t\tif errors.As(err, &known) {{\n\t\t\treturn {ret_zero}{passthrough}\n\t\t}}\n\
         \t\treturn {ret_zero}{wrapped}\n\
         \t}}\n",
        marker = super::super::errors::SDK_ERROR_MARKER,
        passthrough = fail("err".to_string()),
        wrapped = fail(format!(
            "&{contract}{{ContractName: {local:?}, Cause: err}}",
            contract = en.contract,
        )),
    );

    let output_nullable = crate::codegen::ops::op_output_nullable(op);
    let seam = impl_seam_var(n, op);
    let body = if binding.raw {
        raw_body(RawBody {
            op,
            module,
            input_present: input.is_some(),
            output,
            output_nullable,
            symbol: &seam,
            boundary: &boundary,
            ret_zero,
            fail,
            discriminator,
            refs: &mut refs,
        })
    } else {
        let call_args = if input.is_some() { ", input" } else { "" };
        let (bind, tail) = match output {
            Some(_) => ("out, err := ", "\treturn out, nil"),
            None => ("err := ", "\treturn nil"),
        };
        format!("\t{bind}{seam}(ctx, &c.settings{call_args})\n{boundary}{tail}",)
    };

    let doc = doc_of(&op.traits)
        .map(|d| format!("// {}\n", d.replace('\n', "\n// ")))
        .unwrap_or_default();
    Decl::raw_with(
        format!(
            "// {seam} is the call the generated method goes through to reach the\n\
             // bespoke implementation; a test generated from conformance vectors swaps\n\
             // it to simulate an outcome without running the implementation.\n\
             var {seam} = {sym}\n\n\
             {doc}// {method} is implemented by the bespoke {sym}, which lives in this\n\
             // package (drop {module_path} into it):\n\
             //\n\
             //\t{signature}\n\
             func (c *{client}) {sig} {{\n{zero_decl}{validate_block}{body}\n}}",
            method = sig.split('(').next().unwrap_or(sig),
            sym = binding.symbol,
            module_path = binding.module,
            signature = bound_signature(
                binding,
                input.map(go_type),
                output.map(|t| super::go_ret_type(t, output_nullable)),
                n
            ),
            client = n.client,
        ),
        refs,
    )
}

/// The unexported per-operation variable the generated method calls instead of
/// the bespoke symbol directly. Package-level so a `_test.go` file in the same
/// package can swap and restore it.
pub(super) fn impl_seam_var(n: &Names, op: &Shape) -> String {
    super::camel(&format!("{}{}_impl", n.op_prefix, op_local_name(&op.id)))
}

struct RawBody<'a> {
    op: &'a Shape,
    module: &'a Module,
    input_present: bool,
    output: Option<&'a crate::ir::Tref>,
    output_nullable: bool,
    symbol: &'a str,
    boundary: &'a str,
    ret_zero: &'a str,
    fail: &'a dyn Fn(String) -> String,
    discriminator: &'a str,
    refs: &'a mut Vec<Symbol>,
}

/// The raw form: marshal the input to its wire bytes, hand them to the bound
/// symbol, then treat the returned outcome the way a protocol response is
/// treated. A failing outcome discriminates on its code alone (a bespoke
/// implementation has no status to carry), falling back to the generic API
/// error when no declared code matches.
fn raw_body(b: RawBody<'_>) -> String {
    let en = error_names();
    let payload = if b.input_present {
        b.refs.push(import("json", "encoding/json"));
        format!(
            "\tpayload, err := json.Marshal(input)\n\
             \tif err != nil {{\n\t\treturn {ret_zero}{fail_marshal}\n\t}}\n",
            ret_zero = b.ret_zero,
            fail_marshal = (b.fail)(format!(
                "&{contract}{{ContractName: {local:?}, Cause: err}}",
                contract = en.contract,
                local = op_local_name(&b.op.id),
            )),
        )
    } else {
        "\tvar payload []byte\n".to_string()
    };
    let failure = if declared_errors(b.op, b.module).is_empty() {
        // Status 0: a bespoke outcome carries no protocol status, and the body
        // is what the implementation reported.
        format!(
            "&{api}{{Status: 0, Body: string(outcome.Body)}}",
            api = en.api
        )
    } else {
        format!("{}(outcome.Code, outcome.Body)", b.discriminator)
    };
    let success = success_block(
        b.output,
        b.output_nullable,
        b.module,
        &Payload {
            text: "string(outcome.Body)",
            bytes: "outcome.Body",
        },
        b.fail,
        b.refs,
    );
    format!(
        "{payload}\
         \toutcome, err := {sym}(ctx, &c.settings, payload)\n\
         {boundary}\
         \tif !outcome.Success {{\n\t\treturn {ret_zero}{fail_outcome}\n\t}}\n\
         {success}",
        sym = b.symbol,
        boundary = b.boundary,
        ret_zero = b.ret_zero,
        fail_outcome = (b.fail)(failure),
    )
}

/// The Go signature the bespoke symbol must have, rendered for the godoc note
/// above the generated method so the binding is self-documenting.
fn bound_signature(
    binding: &BoundExtension<'_>,
    input: Option<String>,
    output: Option<String>,
    n: &Names,
) -> String {
    let params = if binding.raw {
        format!("ctx context.Context, s *{}, payload []byte", n.settings)
    } else {
        match input {
            Some(ty) => format!("ctx context.Context, s *{}, input {ty}", n.settings),
            None => format!("ctx context.Context, s *{}", n.settings),
        }
    };
    let ret = if binding.raw {
        "(tonoext.Outcome, error)".to_string()
    } else {
        match output {
            Some(ty) => format!("({ty}, error)"),
            None => "error".to_string(),
        }
    };
    format!("func {}({params}) {ret}", binding.symbol)
}

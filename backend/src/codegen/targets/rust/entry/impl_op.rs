//! The Rust glue for an operation implemented by bespoke sources (`ext
//! impl`).
//!
//! Only the typed form is implemented this pass: the bound symbol speaks the
//! operation's own types (`fn(&Settings, Input) -> Result<Output, Box<dyn
//! std::error::Error + Send + Sync>>`, `async` when [`effect_of`] says so)
//! and the glue only guards the boundary, reusing the exact downcast-or-wrap
//! idiom [`crate::codegen::targets::rust::client::wrapper_decls`] already
//! established: a declared `TonoError` passes through typed, anything else
//! becomes a `ContractError` naming the operation.
//!
//! The raw form (a bespoke symbol returning some outcome the glue decodes
//! declaratively, as Go's `tonoext.Outcome` does) has no Rust equivalent yet
//! — that infrastructure is a separate, not-yet-built concern for this
//! target — so a raw-bound operation gets a method that fails loudly at run
//! time instead of silently miscompiling or not compiling at all, exactly
//! like Go/TypeScript's "operation has no implementation" fallback for an
//! entirely unbound operation.

use crate::codegen::entries::op_local_name;
use crate::codegen::extensions::BoundExtension;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::ir::Shape;

use super::use_path;

/// Everything the glue needs that the caller already computed. Neither form
/// implemented this pass (typed, unbound, raw-stub) discriminates declared
/// errors itself, so unlike Go/TypeScript's `impl_op::Method` this carries no
/// `output`/`discriminator`: a typed bespoke fn already returns its declared
/// errors typed, and the raw form (which would need both to decode an
/// outcome declaratively) is not implemented yet.
pub(super) struct Method<'a> {
    pub op: &'a Shape,
    pub name: &'a str,
    pub param: &'a str,
    pub ret: &'a str,
    pub input_ty: Option<&'a str>,
    pub validate_block: &'a str,
    pub binding: Option<&'a BoundExtension<'a>>,
    pub is_async: bool,
    pub doc: &'a str,
    pub refs: &'a mut Vec<Symbol>,
}

pub(super) fn method(m: Method<'_>) -> String {
    let Method {
        op,
        name,
        param,
        ret,
        input_ty,
        validate_block,
        binding,
        is_async,
        doc,
        refs,
    } = m;
    let en = error_names();
    let local = op_local_name(&op.id);
    let effect = if is_async { "async " } else { "" };
    let comma = if param.is_empty() { "" } else { ", " };

    let Some(binding) = binding else {
        return format!(
            "{doc}    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n        Err(TonoError::Contract(ContractError {{ contract_name: {local:?}.to_string(), cause: \"operation has no implementation for Rust\".into() }}))\n    }}",
        );
    };

    if binding.raw {
        return format!(
            "{doc}    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n        Err(TonoError::Contract(ContractError {{ contract_name: {local:?}.to_string(), cause: \"raw bespoke operations are not yet supported for the Rust target\".into() }}))\n    }}",
        );
    }

    refs.push(Symbol::imported(
        binding.symbol,
        use_path(binding.module),
        binding.symbol,
    ));
    let call_args = if input_ty.is_some() {
        "&self.settings, input"
    } else {
        "&self.settings"
    };
    let awaited = if is_async { ".await" } else { "" };
    let signature = bound_signature(binding.symbol, input_ty, ret, is_async);
    format!(
        "{doc}    /// Implemented by the bespoke `{sym}` (drop `{module_path}` into the\n    /// crate at the matching path): `{signature}`.\n    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n{validate_block}        {sym}({call_args}){awaited}.map_err(|cause| match cause.downcast::<{root}>() {{\n            Ok(declared) => *declared,\n            Err(other) => {root}::Contract({contract} {{ contract_name: {local:?}.to_string(), cause: other }}),\n        }})\n    }}",
        sym = binding.symbol,
        module_path = binding.module,
        root = en.root,
        contract = en.contract,
    )
}

fn bound_signature(sym: &str, input_ty: Option<&str>, ret: &str, is_async: bool) -> String {
    let params = match input_ty {
        Some(ty) => format!("settings: &Settings, input: {ty}"),
        None => "settings: &Settings".to_string(),
    };
    let effect = if is_async { "async " } else { "" };
    format!("{effect}fn {sym}({params}) -> Result<{ret}, Box<dyn std::error::Error + Send + Sync>>",)
}

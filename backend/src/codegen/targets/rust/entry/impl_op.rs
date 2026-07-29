//! The Rust glue for an operation implemented by bespoke sources (`ext
//! impl`).
//!
//! Two forms, one error boundary. In the typed form the bound symbol speaks
//! the operation's own types (`fn(&Settings, Input) -> Result<Output, Box<dyn
//! std::error::Error + Send + Sync>>`, `async` when [`effect_of`] says so)
//! and the glue only guards the boundary: a declared `TonoError` passes
//! through typed, anything else becomes a `ContractError` naming the
//! operation. In the raw form (`fn(&Settings, Vec<u8>) -> Result<tono_ext::
//! Outcome, Box<dyn std::error::Error + Send + Sync>>`) the bound symbol
//! returns an outcome and the glue integrates it declaratively, exactly as it
//! integrates a protocol response: decode the success payload strictly into
//! the declared output, or discriminate the failure by its code against the
//! operation's declared error codes (mirrors Go's `tonoext.Outcome`
//! handling).

use crate::codegen::entries::op_local_name;
use crate::codegen::extensions::BoundExtension;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::ir::{Module, Shape, Tref};

use super::decode;
use super::use_path;

/// Everything the glue needs that the caller already computed: the typed
/// form never reads `module`/`output`/`discriminator` (its declared errors
/// already ride the bound symbol's own `Result`), but the raw form needs all
/// three to decode an outcome the same way a protocol response decodes one.
pub(super) struct Method<'a> {
    pub op: &'a Shape,
    pub module: &'a Module,
    pub name: &'a str,
    pub param: &'a str,
    pub ret: &'a str,
    pub input_ty: Option<&'a str>,
    pub output: Option<&'a Tref>,
    /// The name of the already-emitted code-only discrimination function
    /// (`surface::discriminator_decls_for` generates it for any raw binding
    /// with declared errors); `None` when the operation declares none, in
    /// which case a failing outcome always resolves to `Undeclared`.
    pub discriminator: Option<&'a str>,
    pub validate_block: &'a str,
    pub binding: Option<&'a BoundExtension<'a>>,
    pub is_async: bool,
    pub doc: &'a str,
    pub refs: &'a mut Vec<Symbol>,
}

pub(super) fn method(m: Method<'_>) -> String {
    let Method {
        op,
        module,
        name,
        param,
        ret,
        input_ty,
        output,
        discriminator,
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

    refs.push(Symbol::imported(
        binding.symbol,
        use_path(binding.module),
        binding.symbol,
    ));
    let awaited = if is_async { ".await" } else { "" };

    if binding.raw {
        let signature = bound_raw_signature(binding.symbol, is_async);
        let body = raw_body(RawBody {
            module,
            input_ty,
            output,
            discriminator,
            symbol: binding.symbol,
            local,
            en: &en,
            awaited,
        });
        return format!(
            "{doc}    /// Implemented by the bespoke `{sym}` (drop `{module_path}` into the\n    /// crate at the matching path): `{signature}`.\n    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n{validate_block}{body}\n    }}",
            sym = binding.symbol,
            module_path = binding.module,
        );
    }

    let call_args = if input_ty.is_some() {
        "&self.settings, input"
    } else {
        "&self.settings"
    };
    let signature = bound_signature(binding.symbol, input_ty, ret, is_async);
    format!(
        "{doc}    /// Implemented by the bespoke `{sym}` (drop `{module_path}` into the\n    /// crate at the matching path): `{signature}`.\n    pub {effect}fn {name}(&self{comma}{param}) -> Result<{ret}, TonoError> {{\n{validate_block}        {sym}({call_args}){awaited}.map_err(|cause| match cause.downcast::<{root}>() {{\n            Ok(declared) => *declared,\n            Err(other) => {root}::Contract({contract} {{ contract_name: {local:?}.to_string(), cause: other }}),\n        }})\n    }}",
        sym = binding.symbol,
        module_path = binding.module,
        root = en.root,
        contract = en.contract,
    )
}

struct RawBody<'a> {
    module: &'a Module,
    input_ty: Option<&'a str>,
    output: Option<&'a Tref>,
    discriminator: Option<&'a str>,
    symbol: &'a str,
    local: &'a str,
    en: &'a crate::codegen::ops::ErrorNames,
    awaited: &'a str,
}

/// The raw form's body: marshal the input to its wire bytes, hand them to
/// the bound symbol, then treat the returned outcome the way a protocol
/// response is treated. A failing outcome discriminates on its code alone (a
/// bespoke implementation has no status to carry), falling back to the
/// generic API error (status 0) when no declared code matches or none is
/// declared at all.
fn raw_body(b: RawBody<'_>) -> String {
    let en = b.en;
    let payload = if b.input_ty.is_some() {
        "        let payload = serde_json::to_vec(&input).map_err(|e| TonoError::Decode(DecodeError { path: \"$\".to_string(), expected: \"input\".to_string(), raw: e.to_string() }))?;\n"
            .to_string()
    } else {
        "        let payload: Vec<u8> = Vec::new();\n".to_string()
    };
    let boundary = format!(
        "        let outcome = {sym}(&self.settings, payload){awaited}.map_err(|cause| match cause.downcast::<{root}>() {{\n            Ok(declared) => *declared,\n            Err(other) => {root}::Contract({contract} {{ contract_name: {local:?}.to_string(), cause: other }}),\n        }})?;\n",
        sym = b.symbol,
        awaited = b.awaited,
        root = en.root,
        contract = en.contract,
        local = b.local,
    );
    let failure = match b.discriminator {
        Some(disc) => format!("Err({disc}(Some(&outcome.code), &raw_body))"),
        None => format!(
            "Err({root}::Api({failure}::Undeclared({api} {{ status: 0, body: raw_body }})))",
            root = en.root,
            failure = en.api_failure,
            api = en.api,
        ),
    };
    let success = decode::success_block(b.output, b.module, "&raw_body");
    format!(
        "{payload}{boundary}        let raw_body = String::from_utf8_lossy(&outcome.body).into_owned();\n        if !outcome.success {{\n            return {failure};\n        }}\n        {success}",
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

fn bound_raw_signature(sym: &str, is_async: bool) -> String {
    let effect = if is_async { "async " } else { "" };
    format!("{effect}fn {sym}(settings: &Settings, payload: Vec<u8>) -> Result<tono_ext::Outcome, Box<dyn std::error::Error + Send + Sync>>")
}

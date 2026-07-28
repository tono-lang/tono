//! Boundary-wrapping glue for bespoke extensions in the Rust target.
//!
//! Rust has no concrete HTTP client yet (that lands with the Rust runtime), so
//! there is no transport point to invoke lifecycle hooks at. What the Rust target
//! can emit today is the typed boundary wrapper a bound contract needs: it calls
//! the bound symbol and maps a non-declared failure to a `ContractError` while a
//! declared `TonoError` passes through. The concrete client will call these
//! wrappers once it exists.

use crate::codegen::extensions::bound_extensions;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::syntax::render_type;
use crate::codegen::targets::rust::render::RustRules;
use crate::codegen::targets::rust::types::type_expr_of;
use crate::codegen::tree::Decl;
use crate::ir::{ExtKind, Module};

/// The binding-language key the Rust target reads.
const BINDING_LANGS: [&str; 1] = ["rust"];

/// Turn a binding file path into a `crate::` module path: `ext/rust/sign.rs`
/// becomes `ext::rust::sign` (the render rule prepends `crate::`). This makes the
/// binding path a layout requirement on the consumer: the bespoke file must sit at
/// that path under the crate root (`src/ext/rust/sign.rs`) and be declared in the
/// module tree, so `crate::ext::rust::sign` resolves.
fn use_path(module: &str) -> String {
    module
        .strip_suffix(".rs")
        .unwrap_or(module)
        .replace('/', "::")
}

/// One boundary wrapper per bound contract/constraint that declares a typed
/// signature. Hooks are skipped: their input/output are runtime types the Rust
/// runtime has not introduced yet.
pub fn wrapper_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    bound_extensions(module, &BINDING_LANGS)
        .iter()
        .filter(|e| e.kind != ExtKind::Hook)
        .filter_map(|e| {
            let sig = e.signature?;
            let input = render_type(&type_expr_of(&sig.input), &RustRules::default());
            let output = render_type(&type_expr_of(&sig.output), &RustRules::default());
            let text = format!(
                "/// Boundary wrapper for the `{name}` contract: a declared error passes\n\
                 /// through typed; any other failure becomes a {contract}. The concrete\n\
                 /// client that invokes this lands with the Rust HTTP runtime.\n\
                 ///\n\
                 /// The bound `{sym}` must return `Result<{output}, Box<dyn std::error::Error\n\
                 /// + Send + Sync>>`: a `{root}` cause passes through, anything else is\n\
                 /// wrapped (binding-vs-signature validation is deferred).\n\
                 pub fn wrapped_{name}(input: {input}) -> Result<{output}, {root}> {{\n\
                 \x20   {sym}(input).map_err(|cause| match cause.downcast::<{root}>() {{\n\
                 \x20       Ok(declared) => *declared,\n\
                 \x20       Err(other) => {root}::Contract({contract} {{\n\
                 \x20           contract_name: \"{name}\".to_string(),\n\
                 \x20           cause: other,\n\
                 \x20       }}),\n\
                 \x20   }})\n\
                 }}",
                name = e.name,
                sym = e.symbol,
                root = n.root,
                contract = n.contract,
            );
            let refs = vec![Symbol::imported(e.symbol, use_path(e.module), e.symbol)];
            Some(Decl::raw_with(text, refs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::rust::rust_casing;
    use crate::codegen::targets::rust::RustRules;
    use crate::codegen::test_support::rendered;
    use crate::ir::{ExtKind, Extension, Prim, Signature, Tref};

    fn module_with(ext: Extension) -> Module {
        Module {
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![ext],
        }
    }

    fn contract(name: &str, target: &str) -> Extension {
        Extension {
            name: name.into(),
            kind: ExtKind::Contract,
            signature: Some(Signature {
                input: Tref::Prim(Prim::String),
                output: Tref::Prim(Prim::String),
            }),
            raw: false,
            bindings: [("rust".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: Some("v.json".into()),
        }
    }

    #[test]
    fn a_contract_wrapper_maps_err_to_contract_error() {
        let module = module_with(contract("sign_request", "ext/rust/sign.rs#sign_request"));
        let out = rendered(&wrapper_decls(&module), &RustRules::default());
        assert!(out
            .contains("pub fn wrapped_sign_request(input: String) -> Result<String, TonoError> {"));
        // The idiom: map_err with a declared-pass-through, else a ContractError.
        assert!(out
            .contains("sign_request(input).map_err(|cause| match cause.downcast::<TonoError>() {"));
        assert!(out.contains("Ok(declared) => *declared,"));
        assert!(out.contains("Err(other) => TonoError::Contract(ContractError {"));
        assert!(out.contains("contract_name: \"sign_request\".to_string(),"));
    }

    #[test]
    fn a_hook_emits_no_rust_wrapper() {
        // A hook's input/output are runtime types Rust does not have yet.
        let mut ext = contract("before_request", "ext/rust/a.rs#f");
        ext.kind = ExtKind::Hook;
        ext.signature = None;
        assert!(wrapper_decls(&module_with(ext)).is_empty());
    }

    #[test]
    fn an_unbound_target_emits_nothing() {
        let mut ext = contract("sign", "ext/ts/s.ts#sign");
        ext.bindings = [("ts".to_string(), "ext/ts/s.ts#sign".to_string())]
            .into_iter()
            .collect();
        assert!(wrapper_decls(&module_with(ext)).is_empty());
        let _ = rust_casing();
    }
}

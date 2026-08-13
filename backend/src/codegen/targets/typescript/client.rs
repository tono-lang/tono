//! Shared TypeScript codegen helpers the entry client reuses: the
//! import-specifier spelling every bound symbol (hook, contract, impl)
//! resolves through, and the boundary wrapper a bound contract/constraint
//! gets so its binding is never silently unused.
//!
//! A loose (non-entry) operation carries no concrete client in any
//! target, TypeScript included: generation rejects a wire-bound loose
//! operation outright, so the surviving loose-op surface is a bespoke
//! (`ext impl`-bound) trait/interface only, the same shape as Rust/Go.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::extensions::BoundExtension;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::typescript::render::TsRules;
use crate::codegen::targets::typescript::types::type_expr_of;
use crate::ir::{ExtKind, Module};

/// A self-module type symbol, imported from the types file via the serde file's
/// companion redirect (mirrors `errors.rs`).
fn module_symbol(name: &str, module: &Module) -> Symbol {
    Symbol::imported(name.to_string(), module.name.clone(), name.to_string())
}

/// A bound file path (`ext/ts/auth.ts`) as a TypeScript import specifier. The
/// extension is dropped, matching the SDK's own extensionless imports and staying
/// valid where a `.ts` specifier is not (NodeNext/ESM).
///
/// The path is written relative to the SDK's output root, which is not where the
/// importing file sits: a module's groups live in a directory named for it, so
/// the specifier climbs back out to the root first. `from_module` is the IR
/// module doing the importing, and its dotted depth is how far that is. An
/// absolute path is left alone.
pub(crate) fn import_specifier(module: &str, from_module: &str) -> String {
    let path = module
        .strip_suffix(".ts")
        .or_else(|| module.strip_suffix(".tsx"))
        .unwrap_or(module);
    if path.starts_with('/') {
        return path.to_string();
    }
    let up = "../".repeat(from_module.split('.').count());
    let path = path.trim_start_matches("./");
    format!("{up}{path}")
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
/// through. Hooks are handled by the entry's own `transport_hook_wrappers`
/// (their fixed slots carry runtime-typed signatures); a contract/constraint
/// carries a user-typed one, so without this a ts-bound contract would
/// generate no code and its binding would be silently unused.
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
            import_specifier(e.module, &module.name),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::extensions::bound_extensions;
    use crate::ir::{Prim, Signature, Tref};

    fn contract(name: &str, target: &str) -> crate::ir::Extension {
        crate::ir::Extension {
            name: name.into(),
            kind: ExtKind::Contract,
            signature: Some(Signature {
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

    fn module_with(extensions: Vec<crate::ir::Extension>) -> Module {
        Module {
            tests: vec![],
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions,
            ext_libs: vec![],
        }
    }

    #[test]
    fn a_bound_contract_emits_a_ts_boundary_wrapper() {
        let module = module_with(vec![contract("sign_request", "ext/ts/sign.ts#signRequest")]);
        let bound = bound_extensions(&module, &["ts", "typescript"]);
        let mut refs = Vec::new();
        let out = contract_wrappers(&bound, &module, &mut refs);

        assert!(out.contains("function wrappedSignRequest(input: string): string {"));
        assert!(out.contains("return signRequest(input);"));
        // The pass-through-vs-wrap idiom, same as the hook wrappers.
        assert!(out.contains("if (e instanceof TonoError) throw e;"));
        assert!(out.contains("throw new ContractError(\"sign_request\", e);"));
    }

    #[test]
    fn no_bound_contract_emits_nothing() {
        let module = module_with(vec![]);
        let bound = bound_extensions(&module, &["ts", "typescript"]);
        let mut refs = Vec::new();
        assert!(contract_wrappers(&bound, &module, &mut refs).is_empty());
        assert!(refs.is_empty());
    }

    #[test]
    fn import_specifier_climbs_out_of_the_importing_module_directory() {
        assert_eq!(
            import_specifier("ext/ts/auth.ts", "payments.charges"),
            "../../ext/ts/auth"
        );
        assert_eq!(import_specifier("/abs/path.ts", "payments"), "/abs/path");
    }
}

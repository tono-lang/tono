//! Boundary-wrapping glue for bespoke extensions in the Go target.
//!
//! Go has no concrete HTTP client yet (that lands with the Go runtime), so there
//! is no transport point to invoke lifecycle hooks at. What the Go target can
//! emit today is the typed boundary wrapper a bound contract needs: it calls the
//! bound symbol and, on the error path, returns a `ContractError` unless the
//! error is already one (avoiding a double wrap). The concrete client will call
//! these wrappers, and carry the fuller declared-error pass-through, once it
//! exists.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::extensions::bound_extensions;
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::go::render::GoRules;
use crate::codegen::targets::go::types::type_expr_of;
use crate::codegen::tree::Decl;
use crate::ir::{ExtKind, Module};

/// The binding-language key the Go target reads.
const BINDING_LANGS: [&str; 1] = ["go"];

/// An exported Go identifier (PascalCase) for a canonical snake_case name.
fn exported(name: &str) -> String {
    transform(
        name,
        SymbolKind::Type,
        &CasingConfig::new(CaseStyle::Pascal),
        None,
    )
}

/// One boundary wrapper per bound contract/constraint that declares a typed
/// signature. Hooks are skipped: their input/output are runtime types the Go
/// runtime has not introduced yet.
pub fn wrapper_decls(module: &Module) -> Vec<Decl> {
    bound_extensions(module, &BINDING_LANGS)
        .iter()
        .filter(|e| e.kind != ExtKind::Hook)
        .filter_map(|e| {
            let sig = e.signature?;
            // The wrapper's signature types are same-package (no cross-module
            // selector), so the default render rules suffice.
            let rules = GoRules::default();
            let input = render_type(&type_expr_of(&sig.input), &rules);
            let output = render_type(&type_expr_of(&sig.output), &rules);
            let fname = format!("Wrapped{}", exported(e.name));
            let text = format!(
                "// {fname} is the boundary wrapper for the {name} contract: an existing\n\
                 // SDK error passes through, any other failure becomes a ContractError. The\n\
                 // concrete client (with the Go HTTP runtime) carries the fuller declared\n\
                 // pass-through. The bespoke {sym} lives in this package (drop {module} into\n\
                 // it): Go's last path segment is often a keyword, so there is no importable\n\
                 // subpackage name.\n\
                 func {fname}(input {input}) ({output}, error) {{\n\
                 \tout, err := {sym}(input)\n\
                 \tif err != nil {{\n\
                 \t\tvar known {marker}\n\
                 \t\tif errors.As(err, &known) {{\n\
                 \t\t\treturn out, err\n\
                 \t\t}}\n\
                 \t\treturn out, &ContractError{{ContractName: \"{name}\", Cause: err}}\n\
                 \t}}\n\
                 \treturn out, nil\n\
                 }}",
                name = e.name,
                sym = e.symbol,
                module = e.module,
                marker = super::errors::SDK_ERROR_MARKER,
            );
            // Only "errors" is imported: unlike Rust's hierarchical modules, the
            // bespoke Go symbol is same-package (the wrapper calls it unqualified),
            // so the binding path guides where the user drops the file, not an import.
            let refs = vec![Symbol::imported("errors", "errors", "errors")];
            Some(Decl::raw_with(text, refs))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
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
            bindings: [("go".to_string(), target.to_string())]
                .into_iter()
                .collect(),
            conformance: Some("v.json".into()),
        }
    }

    #[test]
    fn a_contract_wrapper_checks_err_and_returns_contract_error() {
        let module = module_with(contract("sign_request", "ext/go/sign.go#SignRequest"));
        let out = rendered(&wrapper_decls(&module), &GoRules::default());
        assert!(out.contains("func WrappedSignRequest(input string) (string, error) {"));
        // The idiom: an explicit error check that returns a ContractError.
        assert!(out.contains("out, err := SignRequest(input)"));
        assert!(out.contains("if err != nil {"));
        // Any SDK error (implementing the sealed marker) passes through; only a
        // foreign error becomes a ContractError.
        assert!(out.contains("var known interface{ sdkError() }"));
        assert!(out.contains("if errors.As(err, &known) {"));
        assert!(
            out.contains("return out, &ContractError{ContractName: \"sign_request\", Cause: err}")
        );
        let _ = go_casing();
    }

    #[test]
    fn a_hook_emits_no_go_wrapper() {
        let mut ext = contract("before_request", "ext/go/a.go#F");
        ext.kind = ExtKind::Hook;
        ext.signature = None;
        assert!(wrapper_decls(&module_with(ext)).is_empty());
    }
}

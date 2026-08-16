//! Boundary-wrapping glue for bespoke extensions in the Go target.
//!
//! Go has no concrete HTTP client yet (that lands with the Go runtime). What
//! the Go target can emit today is the typed boundary wrapper a bound
//! contract needs: it calls the bound symbol and, on the error path, returns
//! a `ContractError` unless the error is already one (avoiding a double
//! wrap). The concrete client will call these wrappers, and carry the fuller
//! declared-error pass-through, once it exists.

use crate::codegen::casing::{transform, CaseStyle, CasingConfig};
use crate::codegen::extensions::bound_extensions;
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::syntax::render_type;
use crate::codegen::targets::go::render::GoRules;
use crate::codegen::targets::go::types::type_expr_of;
use crate::codegen::tree::Decl;
use crate::ir::Module;

/// The binding-language key the Go target reads.
const BINDING_LANGS: [&str; 1] = ["go"];

/// Whether the module binds any bespoke Go code. That is what puts a boundary
/// in the package (a contract wrapper, a bound operation), and the boundary
/// is the only thing that has to tell an SDK error from a foreign one.
pub fn binds_bespoke(module: &Module) -> bool {
    !bound_extensions(module, &BINDING_LANGS).is_empty()
}

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
/// signature.
pub fn wrapper_decls(module: &Module) -> Vec<Decl> {
    bound_extensions(module, &BINDING_LANGS)
        .iter()
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
            tests: vec![],
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![ext],
            ext_libs: vec![],
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
    fn the_marker_is_emitted_exactly_when_something_binds_bespoke_code() {
        // The marker exists to be matched at a boundary, and only a binding puts
        // a boundary in the package. It is unexported, so no consumer outside the
        // package could have matched it either.
        let bound = module_with(contract("sign_request", "ext/go/sign.go#SignRequest"));
        assert!(binds_bespoke(&bound));
        let all_live = crate::codegen::taxonomy::TaxonomyLiveness::all_live();
        let taxonomy = rendered(
            &super::super::errors::taxonomy_and_declared_decls(&bound, &all_live),
            &GoRules::default(),
        );
        assert!(taxonomy.contains("func (e *ValidationError) sdkError() {}"));
        assert!(taxonomy.contains("func (e *ContractError) sdkError() {}"));

        let mut unbound = bound.clone();
        unbound.extensions[0].bindings.clear();
        assert!(!binds_bespoke(&unbound));
        let taxonomy = rendered(
            &super::super::errors::taxonomy_and_declared_decls(&unbound, &all_live),
            &GoRules::default(),
        );
        assert!(!taxonomy.contains("sdkError()"));
        // The categories themselves are unchanged: only the marker goes.
        assert!(taxonomy.contains("type ContractError struct {"));
    }
}

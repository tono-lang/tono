//! The Go error surface and client interface: the closed taxonomy as error
//! values (no invented root type, only the stdlib `error` interface), declared
//! operation errors made into error values on their existing structs, the
//! per-operation discrimination function, and the blocking client interface.
//!
//! Go discriminates with `errors.As`, so each category is a distinct struct
//! implementing `error`; the transport and contract categories `Unwrap` their
//! native cause. A declared error stays the struct the types file already
//! emits, the methods added here (`Error`, `Retryable`) are what make it an
//! error value.

use crate::codegen::casing::{transform, CasingConfig};
use crate::codegen::ops::{
    self, error_names, error_type_name, module_declared_errors, DeclaredError, ErrorNames,
};
use crate::codegen::symbol::{Symbol, SymbolKind};
use crate::codegen::targets::go::types::{go_casing, type_expr_of, LANG};
use crate::codegen::taxonomy::TaxonomyLiveness;
use crate::codegen::tree::Decl;
use crate::ir::{Module, Shape};

/// The anonymous marker interface a bound extension's boundary wrapper
/// matches with `errors.As` to tell an SDK-emitted error (preserve it as-is)
/// from a foreign one (wrap it as a ContractError). Every generated error
/// value carries the unexported `sdkError()` method, so only this package's
/// types satisfy it and the taxonomy stays sealed.
pub const SDK_ERROR_MARKER: &str = "interface{ sdkError() }";

/// The unexported marker method that makes a generated error value part of the
/// sealed SDK taxonomy (see [`SDK_ERROR_MARKER`]).
///
/// Only a boundary wrapper matches it, and only bespoke bindings produce one, so
/// `sealed` is false for a module that binds nothing and the method is left out
/// rather than shipped as a method nothing can call. It is unexported, so no
/// consumer outside the package could have matched it either.
fn marker_method(sealed: bool, name: &str) -> Option<Decl> {
    sealed.then(|| Decl::raw(format!("func (e *{name}) sdkError() {{}}")))
}

/// The declarations for the types file: the taxonomy error values, the
/// declared errors' methods, and the blocking client interface.
///
/// A loose (non-entry) operation gets only a blocking `interface` (no
/// concrete client), so nothing in the generated SDK constructs any category
/// either way; the full taxonomy is vocabulary the interface implementer may
/// need, not dead code, hence [`TaxonomyLiveness::all_live`] rather than a
/// derived liveness.
pub fn type_decls(module: &Module, config: &CasingConfig) -> Vec<Decl> {
    let mut decls = taxonomy_and_declared_decls(module, &TaxonomyLiveness::all_live());
    // The error channel is the native (T, error) pair on every method.
    decls.push(ops::client_decl(
        module,
        config,
        LANG,
        &type_expr_of,
        Some("error"),
    ));
    decls
}

/// The taxonomy and the declared errors' methods without the loose-op client
/// interface: what an entry-only module needs (its client surface is the
/// entry's own struct and mock interface), gated to what `liveness` reports
/// that module's entries can actually construct for this target.
pub fn taxonomy_and_declared_decls(module: &Module, liveness: &TaxonomyLiveness) -> Vec<Decl> {
    let sealed = super::client::binds_bespoke(module);
    let mut decls = taxonomy_decls(sealed, liveness);
    decls.extend(declared_error_decls(module, sealed));
    decls
}

/// The declarations for the serde file: one discrimination function per
/// operation that declares errors.
pub fn serde_decls(module: &Module) -> Vec<Decl> {
    let n = error_names();
    ops::discriminator_decls(module, |op, ordered| discriminator_fn(op, ordered, &n))
}

/// The `Violation` record on its own: the field/constraint/message triple a
/// validator appends. The full taxonomy embeds it, so it is only emitted standalone
/// for a module that has constraints but no operations (hence no taxonomy).
pub fn violation_decl() -> Decl {
    let n = error_names();
    Decl::raw(format!(
        "type {} struct {{\n\tField      string `json:\"field\"`\n\tConstraint string `json:\"constraint\"`\n\tMessage    string `json:\"message\"`\n}}",
        n.violation
    ))
}

/// The wire message of a declared error: its body code when declared, else its
/// canonical snake name.
fn declared_message(err: &DeclaredError) -> String {
    err.code
        .as_ref()
        .map(|c| c.value.clone())
        .unwrap_or_else(|| {
            err.shape_id
                .rsplit('#')
                .next()
                .unwrap_or(&err.shape_id)
                .to_string()
        })
}

/// The `Validation` category a validator returns: the error struct carrying its
/// collected violations, the `Error` method that makes it an error value, and the
/// marker that keeps it inside the sealed SDK taxonomy.
fn validation_category_decls(sealed: bool) -> Vec<Decl> {
    let n = error_names();
    let mut decls = vec![
        Decl::raw(format!(
            "type {} struct {{\n\tViolations []{} `json:\"violations\"`\n}}",
            n.validation, n.violation
        )),
        Decl::raw(format!(
            "func (e *{}) Error() string {{ return \"validation failed\" }}",
            n.validation
        )),
    ];
    decls.extend(marker_method(sealed, &n.validation));
    decls
}

/// The declarations a validator needs when the module has constraints but no
/// operations (hence no full taxonomy): the `Violation` record and the
/// `Validation` category itself.
pub fn standalone_validation_decls(sealed: bool) -> Vec<Decl> {
    let mut decls = vec![violation_decl()];
    decls.extend(validation_category_decls(sealed));
    decls
}

/// The closed error taxonomy as error values, gated to the categories
/// `liveness` reports as reachable: a category nothing generated for this
/// module can construct gets no struct, no `Error`/`Unwrap` methods, and no
/// marker. Go has no hierarchy to root, so each category stands alone; unlike
/// Rust's single self-referential enum, gating one out never requires
/// touching another category's declarations.
fn taxonomy_decls(sealed: bool, liveness: &TaxonomyLiveness) -> Vec<Decl> {
    let n = error_names();
    let error_method = |name: &str, message: &str| {
        Decl::raw(format!(
            "func (e *{name}) Error() string {{ return {message} }}"
        ))
    };
    let unwrap_method = |name: &str| {
        Decl::raw(format!(
            "func (e *{name}) Unwrap() error {{ return e.Cause }}"
        ))
    };
    // Each category is a struct plus its error methods plus the marker; the
    // marker is what makes the set closed, so it rides the same emission rather
    // than being appended out of order.
    let category = |decls: &mut Vec<Decl>, name: &str, items: Vec<Decl>| {
        decls.extend(items);
        decls.extend(marker_method(sealed, name));
    };
    let mut decls = Vec::new();
    if liveness.validation {
        decls.extend(standalone_validation_decls(sealed));
    }
    if liveness.transport {
        category(
            &mut decls,
            &n.transport,
            vec![
                Decl::raw(format!("type {} struct {{\n\tCause error\n}}", n.transport)),
                error_method(&n.transport, "\"transport failure\""),
                unwrap_method(&n.transport),
            ],
        );
    }
    if liveness.decode {
        category(
            &mut decls,
            &n.decode,
            vec![
                Decl::raw(format!(
                    "type {} struct {{\n\tPath     string\n\tExpected string\n\tRaw      string\n}}",
                    n.decode
                )),
                error_method(
                    &n.decode,
                    "\"response body did not match the declared schema\"",
                ),
            ],
        );
    }
    if liveness.contract {
        category(
            &mut decls,
            &n.contract,
            vec![
                Decl::raw(format!(
                    "type {} struct {{\n\tContractName string\n\tCause        error\n}}",
                    n.contract
                )),
                error_method(
                    &n.contract,
                    "\"contract '\" + e.ContractName + \"' failed\"",
                ),
                unwrap_method(&n.contract),
            ],
        );
    }
    if liveness.api {
        category(
            &mut decls,
            &n.api,
            vec![
                Decl::raw(format!(
                    "type {} struct {{\n\tStatus int\n\tBody   string\n}}",
                    n.api
                )),
                Decl::raw_with(
                    format!(
                        "func (e *{}) Error() string {{ return \"api error \" + strconv.Itoa(e.Status) }}",
                        n.api
                    ),
                    vec![Symbol::imported("strconv", "strconv", "strconv")],
                ),
            ],
        );
    }
    if liveness.config {
        // Construction failures (a required source that resolved to nothing)
        // ride their own category so a caller can tell a misconfigured client
        // from a request or transport failure. Cause unwraps the source's own
        // failure the message wraps in, the same way Transport and Contract do.
        category(
            &mut decls,
            &n.config,
            vec![
                Decl::raw(format!(
                    "type {} struct {{\n\tMessage string\n\tCause   error\n}}",
                    n.config
                )),
                error_method(&n.config, "e.Message"),
                unwrap_method(&n.config),
            ],
        );
    }
    decls
}

/// The methods that make each declared error struct an error value: `Error`
/// (its body code, or its canonical name) and the `Retryable` predicate from
/// `@retryable`. The status and `@errorCode` a spec declares are constants
/// beside the type they describe (see [`declared_error_const_decl`]), not
/// literals in the decoder that happens to consult them.
fn declared_error_decls(module: &Module, sealed: bool) -> Vec<Decl> {
    module_declared_errors(module)
        .iter()
        .flat_map(|err| {
            let ty = error_type_name(err);
            let mut decls: Vec<Decl> = declared_error_const_decl(err).into_iter().collect();
            decls.push(Decl::raw(format!(
                "func (e *{ty}) Error() string {{ return \"{}\" }}",
                declared_message(err)
            )));
            decls.push(Decl::raw(format!(
                "func (e *{ty}) Retryable() bool {{ return {} }}",
                err.retryable
            )));
            decls.extend(marker_method(sealed, &ty));
            decls
        })
        .collect()
}

/// The named constants a declared error's own status and `@errorCode` become,
/// declared beside the type they describe. `None` for the (frontend-rejected
/// in practice) case of an error with no status.
fn declared_error_const_decl(err: &DeclaredError) -> Option<Decl> {
    let ty = error_type_name(err);
    let status = err.status?;
    Some(match &err.code {
        Some(code) => Decl::raw(format!(
            "const status{ty} = {status}\nconst code{ty} = {:?}",
            code.value
        )),
        None => Decl::raw(format!("const status{ty} = {status}")),
    })
}

fn status_const(ty: &str) -> String {
    format!("status{ty}")
}

fn code_const(ty: &str) -> String {
    format!("code{ty}")
}

/// One node of the probe's nested-struct shape, built by merging every
/// declared error's `@errorCode` path so a shared prefix (two errors probing
/// under `"error"`) becomes one nested struct instead of two. Children keep
/// insertion order so the emitted struct is deterministic and reads in the
/// order the errors were declared.
#[derive(Default)]
struct ProbeNode {
    children: Vec<(String, ProbeNode)>,
}

impl ProbeNode {
    fn insert(&mut self, path: &[String]) {
        let Some((head, rest)) = path.split_first() else {
            return;
        };
        match self.children.iter_mut().find(|(seg, _)| seg == head) {
            Some((_, child)) => child.insert(rest),
            None => {
                let mut child = ProbeNode::default();
                child.insert(rest);
                self.children.push((head.clone(), child));
            }
        }
    }
}

/// The Go field name a JSON path segment becomes: exported PascalCase so
/// `encoding/json` can see it.
fn probe_field_name(segment: &str) -> String {
    transform(segment, SymbolKind::Field, &go_casing(), None)
}

/// Render the probe struct's body (the lines between `struct {` and `}`) at
/// the given indent depth. A leaf segment becomes a `string` field; a segment
/// that is itself a path prefix becomes a nested anonymous struct.
fn render_probe_node(node: &ProbeNode, depth: usize) -> String {
    let indent = "\t".repeat(depth);
    let mut out = String::new();
    for (seg, child) in &node.children {
        let field = probe_field_name(seg);
        if child.children.is_empty() {
            out.push_str(&format!("{indent}{field} string `json:{seg:?}`\n"));
        } else {
            out.push_str(&format!("{indent}{field} struct {{\n"));
            out.push_str(&render_probe_node(child, depth + 1));
            out.push_str(&format!("{indent}}} `json:{seg:?}`\n"));
        }
    }
    out
}

/// The Go expression reading a declared error's discriminator off the probe:
/// `probe.Error.Type` for `["error", "type"]`.
fn probe_accessor(path: &[String]) -> String {
    let mut expr = "probe".to_string();
    for seg in path {
        expr.push('.');
        expr.push_str(&probe_field_name(seg));
    }
    expr
}

/// One discrimination function: `(status, raw body) -> error`. The mapping
/// tries the declared errors and resolves everything else to the concrete
/// fallback type.
fn discriminator_fn(op: &Shape, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let fn_name = format!(
        "Decode{}Error",
        crate::codegen::conventions::type_ident_from_id(&op.id)
    );
    discriminator_fn_body(&fn_name, ordered, n)
}

/// The same discrimination function under a caller-chosen name (an
/// entry-nested operation derives its name through the entry rule, not from
/// the raw shape id).
pub fn discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    discriminator_fn_body(fn_name, ordered, &error_names())
}

/// The code-only discrimination a bespoke raw outcome uses. There is no protocol
/// status to match on, so a declared error is chosen by its `@errorCode` alone
/// and everything unmatched resolves to the fallback, whose status 0 marks the
/// absence of a protocol status. An error declared without a code can never be
/// selected here: nothing in the outcome identifies it.
pub fn outcome_discriminator_fn_named(fn_name: &str, ordered: &[DeclaredError]) -> Decl {
    let n = error_names();
    let mut body = format!("func {fn_name}(code string, body []byte) error {{\n");
    for err in ordered.iter().filter(|e| e.code.is_some()) {
        let ty = error_type_name(err);
        // A declared match whose body does not unmarshal falls through to the
        // fallback so a new field or a changed shape never breaks the caller.
        body.push_str(&format!(
            "\tif code == {code_const} {{\n\t\tvar data {ty}\n\t\tif json.Unmarshal(body, &data) == nil {{\n\t\t\treturn &data\n\t\t}}\n\t}}\n",
            code_const = code_const(&ty),
        ));
    }
    body.push_str(&format!(
        "\treturn &{}{{Status: 0, Body: string(body)}}\n}}",
        n.api
    ));
    Decl::raw_with(
        body,
        vec![Symbol::imported("json", "encoding/json", "json")],
    )
}

fn discriminator_fn_body(fn_name: &str, ordered: &[DeclaredError], n: &ErrorNames) -> Decl {
    let mut body = String::new();
    body.push_str(&format!(
        "func {fn_name}(status int, body []byte) error {{\n"
    ));
    let coded: Vec<&DeclaredError> = ordered.iter().filter(|e| e.code.is_some()).collect();
    if !coded.is_empty() {
        let mut root = ProbeNode::default();
        for err in &coded {
            root.insert(&err.code.as_ref().unwrap().path);
        }
        // The comment rides into the generated code: a body that fails to
        // unmarshal here leaves the probe zeroed, so no guard below matches
        // and the fallback carries the raw body as the generic ApiError.
        body.push_str(&format!(
            "\tvar probe struct {{\n{}\t}}\n\t// A body that fails to unmarshal leaves probe zeroed: no guard below\n\t// matches, and the fallback carries the raw body as APIError.\n\t_ = json.Unmarshal(body, &probe)\n",
            render_probe_node(&root, 2)
        ));
    }
    for err in ordered {
        let ty = error_type_name(err);
        // A declared error always has a status in practice (the frontend
        // rejects one without); the literal fallback only guards the type
        // against that theoretical gap.
        let status_expr = if err.status.is_some() {
            status_const(&ty)
        } else {
            "0".to_string()
        };
        let guard = match &err.code {
            Some(code) => format!(
                "status == {status_expr} && {accessor} == {code_expr}",
                accessor = probe_accessor(&code.path),
                code_expr = code_const(&ty)
            ),
            None => format!("status == {status_expr}"),
        };
        // A declared match whose body does not unmarshal falls through to the
        // fallback so new server fields or shapes never break the caller.
        body.push_str(&format!(
            "\tif {guard} {{\n\t\tvar data {ty}\n\t\tif json.Unmarshal(body, &data) == nil {{\n\t\t\treturn &data\n\t\t}}\n\t}}\n"
        ));
    }
    body.push_str(&format!(
        "\treturn &{}{{Status: status, Body: string(body)}}\n}}",
        n.api
    ));
    Decl::raw_with(
        body,
        vec![Symbol::imported("json", "encoding/json", "json")],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::go::types::go_casing;
    use crate::codegen::targets::go::GoRules;
    use crate::codegen::test_support::{error_demo_module, error_shape, operation, rendered};

    fn types_text(module: &Module) -> String {
        rendered(&type_decls(module, &go_casing()), &GoRules::default())
    }

    #[test]
    fn the_taxonomy_is_error_values_with_no_invented_root() {
        let out = types_text(&error_demo_module());
        for category in [
            "ValidationError",
            "TransportError",
            "DecodeError",
            "ContractError",
            "APIError",
        ] {
            assert!(out.contains(&format!("type {category} struct {{")));
            assert!(
                out.contains(&format!("func (e *{category}) Error() string {{")),
                "{category} must implement error"
            );
        }
        // No root type: nothing named after the hierarchy root is generated.
        assert!(!out.contains("TonoError"));
        // The transport and contract categories unwrap their native cause.
        assert!(out.contains("func (e *TransportError) Unwrap() error { return e.Cause }"));
        assert!(out.contains("func (e *ContractError) Unwrap() error { return e.Cause }"));
        assert!(out.contains("\tStatus int\n\tBody   string\n"));
    }

    #[test]
    fn declared_errors_gain_error_and_retryable_methods() {
        let out = types_text(&error_demo_module());
        assert!(out
            .contains("func (e *PaymentDeclined) Error() string { return \"payment_declined\" }"));
        assert!(out.contains("func (e *PaymentDeclined) Retryable() bool { return true }"));
        // Without a code the message falls back to the canonical name; without
        // @retryable the predicate reports false.
        assert!(out.contains("func (e *RateLimited) Error() string { return \"rate_limited\" }"));
        assert!(out.contains("func (e *RateLimited) Retryable() bool { return false }"));
    }

    #[test]
    fn the_client_interface_is_blocking_with_the_error_pair() {
        let out = types_text(&error_demo_module());
        assert!(out.contains(
            "type Client interface {\n\tCreateCharge(input ChargeInput) (Charge, error)\n}"
        ));
    }

    #[test]
    fn the_discriminator_probes_the_code_field_and_falls_back() {
        let out = rendered(&serde_decls(&error_demo_module()), &GoRules::default());
        assert!(out.contains("func DecodeCreateChargeError(status int, body []byte) error {"));
        assert!(out.contains("Code string `json:\"code\"`"));
        assert!(out
            .contains("if status == statusPaymentDeclined && probe.Code == codePaymentDeclined {"));
        assert!(out.contains("var data PaymentDeclined"));
        assert!(out.contains("if status == statusRateLimited {"));
        assert!(out.contains("return &APIError{Status: status, Body: string(body)}"));
    }

    #[test]
    fn declared_errors_gain_named_constants_beside_their_type() {
        let out = types_text(&error_demo_module());
        assert!(out.contains("const statusPaymentDeclined = 402"));
        assert!(out.contains("const codePaymentDeclined = \"payment_declined\""));
        assert!(out.contains("const statusRateLimited = 429"));
        assert!(!out.contains("codeRateLimited"));
    }

    #[test]
    fn a_codeless_error_set_skips_the_probe() {
        let mut module = error_demo_module();
        module
            .shapes
            .push(error_shape("m#slow_down", vec![], 503, None, false));
        module.operations = vec![operation("m#fetch", vec![], vec!["m#slow_down"])];
        let out = rendered(&serde_decls(&module), &GoRules::default());
        assert!(!out.contains("probe"));
        assert!(out.contains("if status == statusSlowDown {"));
    }

    #[test]
    fn shared_path_prefixes_merge_into_one_nested_struct() {
        use crate::codegen::test_support::error_shape_at;
        let mut module = error_demo_module();
        module.shapes.push(error_shape_at(
            "m#invalid_type",
            vec![],
            400,
            Some(("error.type", "invalid")),
            false,
        ));
        module.shapes.push(error_shape_at(
            "m#invalid_code",
            vec![],
            400,
            Some(("error.code", "bad")),
            false,
        ));
        module.operations = vec![operation(
            "m#fetch",
            vec![],
            vec!["m#invalid_type", "m#invalid_code"],
        )];
        let out = rendered(&serde_decls(&module), &GoRules::default());
        // A shared "error" prefix collapses into one nested struct, not two
        // separate top-level "Error" fields.
        assert_eq!(out.matches("Error struct {").count(), 1);
        assert!(out.contains("\t\tError struct {\n"));
        assert!(out.contains("\t\t\tType string `json:\"type\"`\n"));
        assert!(out.contains("\t\t\tCode string `json:\"code\"`\n"));
        assert!(out.contains("\t\t} `json:\"error\"`\n"));
    }
}

//! The language-neutral reads every emitted transport makes off a
//! `WireBinding`: which request shape an operation needs is a property of
//! the binding alone, so the decision lives once here and each target
//! spells only its own syntax over the answer.

use crate::ir::{TemplatePart, WireBinding, WirePart};

/// Whether the operation reads any input member individually off a decoded
/// record (a label, query, header, payload, or partial-body position); a
/// whole-body operation serializes the typed/encoded input directly instead.
pub fn needs_record(wire: &WireBinding) -> bool {
    let uri_reads_record = wire
        .uri
        .iter()
        .any(|part| matches!(part, TemplatePart::Input(_)));
    let any_non_body_binding = wire.bindings.values().any(|p| !matches!(p, WirePart::Body));
    uri_reads_record || any_non_body_binding
}

/// Whether any input member is bound to the query string.
pub fn has_query(wire: &WireBinding) -> bool {
    wire.bindings
        .values()
        .any(|p| matches!(p, WirePart::Query { .. }))
}

/// The success test text, common to every target: the 2xx-range convention
/// when the operation left `code:` unset (`wire.success` empty), otherwise an
/// exact match against exactly the declared statuses, joined by `||`. `field`
/// is the target's own status-code expression (e.g. `outcome.status`,
/// `outcome.Status`, `response.status`) and `eq` its equality operator (`==`
/// or `===`); this is the only thing that varies target to target.
pub fn success_test_expr(wire: &WireBinding, field: &str, eq: &str) -> String {
    if wire.success.is_empty() {
        format!("{field} >= 200 && {field} < 300")
    } else {
        wire.success
            .iter()
            .map(|code| format!("{field} {eq} {code}"))
            .collect::<Vec<_>>()
            .join(" || ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire() -> WireBinding {
        WireBinding {
            method: "GET".into(),
            uri: vec![TemplatePart::Lit("/x".into())],
            bindings: Default::default(),
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: Vec::new(),
            timeout: None,
            retry: None,
        }
    }

    #[test]
    fn needs_record_is_false_with_no_bindings_and_false_for_all_body() {
        let mut w = wire();
        assert!(!needs_record(&w));
        w.bindings = [("amount".to_string(), WirePart::Body)]
            .into_iter()
            .collect();
        assert!(!needs_record(&w));
    }

    #[test]
    fn needs_record_is_true_for_a_uri_input_or_a_non_body_binding() {
        let mut w = wire();
        w.uri = vec![TemplatePart::Input("id".into())];
        assert!(needs_record(&w));
        let mut w = wire();
        w.bindings = [("tag".to_string(), WirePart::Query { name: "tag".into() })]
            .into_iter()
            .collect();
        assert!(needs_record(&w));
        assert!(has_query(&w));
    }

    #[test]
    fn success_test_expr_defaults_to_the_2xx_range_alone() {
        assert_eq!(
            success_test_expr(&wire(), "outcome.status", "=="),
            "outcome.status >= 200 && outcome.status < 300"
        );
    }

    #[test]
    fn success_test_expr_is_an_exact_match_against_declared_codes_only() {
        let mut w = wire();
        w.success = vec![200, 404, 202];
        assert_eq!(
            success_test_expr(&w, "outcome.status", "=="),
            "outcome.status == 200 || outcome.status == 404 || outcome.status == 202"
        );
    }

    #[test]
    fn success_test_expr_is_exact_for_a_single_declared_code_inside_2xx() {
        let mut w = wire();
        w.success = vec![201];
        assert_eq!(
            success_test_expr(&w, "outcome.status", "=="),
            "outcome.status == 201"
        );
    }

    #[test]
    fn success_test_expr_spells_the_field_and_operator_verbatim() {
        let mut w = wire();
        w.success = vec![207];
        assert_eq!(
            success_test_expr(&w, "response.status", "==="),
            "response.status === 207"
        );
    }
}

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

/// The declared success statuses outside the 2xx range, in declaration
/// order: any 2xx succeeds even when not literally declared (the rule every
/// target shares), so only these need their own arm in an emitted success
/// test.
pub fn extra_success_codes(wire: &WireBinding) -> impl Iterator<Item = i64> + '_ {
    wire.success
        .iter()
        .copied()
        .filter(|code| !(200..300).contains(code))
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
    fn extra_success_codes_keep_only_the_out_of_range_declarations_in_order() {
        let mut w = wire();
        w.success = vec![200, 404, 202, 302];
        assert_eq!(extra_success_codes(&w).collect::<Vec<_>>(), vec![404, 302]);
    }
}

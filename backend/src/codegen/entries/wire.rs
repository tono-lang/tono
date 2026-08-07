//! The language-neutral reads every emitted transport makes off a
//! `WireBinding`: which request shape an operation needs is a property of
//! the binding alone, so the decision lives once here and each target
//! spells only its own syntax over the answer.

use crate::ir::{TemplatePart, WireBinding, WireValue};

// A param reference with no further segments reads the whole typed input
// value directly (a new capability the named-parameter form adds); one with
// a segment reads a member of the input struct, exactly like the legacy
// `{name}` placeholder does (an op has exactly one parameter, so "member of
// the parameter" and "member of the input" are the same decoded record).
fn part_reads_record(part: &TemplatePart) -> bool {
    match part {
        TemplatePart::Input(_) => true,
        TemplatePart::Param(segs) => !segs.is_empty(),
        TemplatePart::Lit(_) | TemplatePart::Field(_) => false,
    }
}

fn template_reads_record(parts: &[TemplatePart]) -> bool {
    parts.iter().any(part_reads_record)
}

fn value_reads_record(value: &WireValue) -> bool {
    match value {
        WireValue::Param(segs) => !segs.is_empty(),
        WireValue::Template(parts) => template_reads_record(parts),
        WireValue::Object(fields) => fields.iter().any(|(_, v)| value_reads_record(v)),
        WireValue::Lit(_) | WireValue::Field(_) => false,
    }
}

/// Whether the operation reads any input member individually off a decoded
/// record (an op-parameter member reference in the uri/endpoint/header/
/// query/body positions); a whole-body operation serializes the typed/
/// encoded input directly instead.
pub fn needs_record(wire: &WireBinding) -> bool {
    let uri_reads_record = value_reads_record(&wire.uri);
    let endpoint_reads_record = wire.endpoint.as_ref().is_some_and(value_reads_record);
    let kv_reads_record = |kv: &[(Vec<TemplatePart>, WireValue)]| {
        kv.iter()
            .any(|(k, v)| template_reads_record(k) || value_reads_record(v))
    };
    let body_reads_record = wire.body.as_ref().is_some_and(value_reads_record);
    uri_reads_record
        || endpoint_reads_record
        || kv_reads_record(&wire.request_headers)
        || kv_reads_record(&wire.query)
        || body_reads_record
}

/// Whether the operation declares any query-string parameter.
pub fn has_query(wire: &WireBinding) -> bool {
    !wire.query.is_empty()
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
            uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
            body: None,
            response_bindings: Default::default(),
            success: Vec::new(),
            endpoint: None,
            request_headers: Vec::new(),
            query: Vec::new(),
            timeout: None,
            retry: None,
        }
    }

    #[test]
    fn needs_record_is_false_with_no_body_and_false_for_a_whole_param_body() {
        let mut w = wire();
        assert!(!needs_record(&w));
        w.body = Some(WireValue::Param(Vec::new()));
        assert!(!needs_record(&w));
    }

    #[test]
    fn needs_record_is_true_for_a_uri_input_or_a_param_member_body() {
        let mut w = wire();
        w.uri = WireValue::Template(vec![TemplatePart::Input("id".into())]);
        assert!(needs_record(&w));
        let mut w = wire();
        w.body = Some(WireValue::Param(vec!["amount".to_string()]));
        assert!(needs_record(&w));
        let mut w = wire();
        w.query = vec![(
            vec![TemplatePart::Lit("tag".into())],
            WireValue::Param(vec!["tag".to_string()]),
        )];
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

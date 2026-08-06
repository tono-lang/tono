//! The declared-test model the native test emitters read: the `test` blocks
//! the IR carries per module (`Module.tests`), resolved against the entries
//! they exercise.
//!
//! The frontend already typechecks every test, but the generator re-validates
//! cheaply so a hand-authored or third-party IR fails with a named reason
//! instead of generating a test that cannot compile. Validation and planning
//! are one pass ([`entry_tests`]): what the emitters consume is exactly what
//! was checked.
//!
//! The current generation phase covers a single construction, at most one
//! stubbed dependency, and at most one call per test. Anything beyond that
//! (multi-call flows, dataflow between bindings) is a *loud* generation error,
//! never a silently skipped case.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::ir::{
    ExtKind, FieldPattern, Model, Module, RequestPattern, Shape, ShapeKind, ShapePattern,
    StubAnswer, StubDep, TestCall, TestConstruction, TestDecl, TestExpect, TestPattern, TestStub,
};

/// The bare operation name of an entry op id (`notes#client.fetch_note` ->
/// `fetch_note`).
pub fn bare_op_name(op_id: &str) -> &str {
    let local = op_id.rsplit('#').next().unwrap_or(op_id);
    local.rsplit('.').next().unwrap_or(local)
}

fn bare_entry_name(entry_id: &str) -> &str {
    entry_id.rsplit('#').next().unwrap_or(entry_id)
}

/// One declared test resolved against its entry, ready for an emitter.
#[derive(Debug)]
pub struct PlannedTest<'a> {
    pub name: &'a str,
    pub construction: &'a TestConstruction,
    /// The stub covering the call, when the test pins the dependency.
    pub stub: Option<&'a TestStub>,
    pub call: Option<&'a TestCall>,
    /// The operation shape of the call, resolved in the entry.
    pub op: Option<&'a Shape>,
    /// The pattern over the call's outcome (or over the construction's, for a
    /// construction-only test).
    pub outcome: Option<&'a TestPattern>,
    /// The request patterns over the stub's recorded requests: the whole list
    /// matches all recorded requests, in order, with equal length.
    pub requests: Option<&'a [RequestPattern]>,
    /// A call with its dependency stubbed is hermetic; one against the real
    /// dependency is live; a construction-only test is hermetic.
    pub hermetic: bool,
}

/// The declared tests of one entry, in declaration order.
#[derive(Debug)]
pub struct EntryTests<'a> {
    /// The entry's bare local name (matches `EntryModel::name`).
    pub entry: String,
    pub tests: Vec<PlannedTest<'a>>,
}

/// The entries the module declares tests for, by bare name. This is what the
/// client seams (transport constructor variant, impl swapper) key on: they are
/// emitted only when the entry has tests, before any validation runs.
pub fn entries_with_tests(module: &Module) -> BTreeSet<String> {
    module
        .tests
        .iter()
        .flat_map(|t| &t.constructions)
        .map(|c| c.entry.clone())
        .collect()
}

/// Resolve and validate every declared test of the module, grouped by entry in
/// first-use order. The error message names the test, so a bad case is a clear
/// refusal instead of a generated test that cannot compile.
pub fn entry_tests(module: &Module) -> Result<Vec<EntryTests<'_>>, String> {
    let mut groups: Vec<EntryTests<'_>> = Vec::new();
    for test in &module.tests {
        let planned = plan_test(module, test)
            .map_err(|e| format!("test '{}' in module '{}': {e}", test.name, module.name))?;
        let entry = planned.construction.entry.clone();
        match groups.iter_mut().find(|g| g.entry == entry) {
            Some(group) => group.tests.push(planned),
            None => groups.push(EntryTests {
                entry,
                tests: vec![planned],
            }),
        }
    }
    Ok(groups)
}

/// Validate every module's declared tests; the pipeline runs this before any
/// emitter reads them.
pub fn validate_declared_tests(model: &Model) -> Result<(), String> {
    for module in &model.modules {
        entry_tests(module)?;
    }
    Ok(())
}

/// The entry shape with the given bare name.
fn entry_shape<'a>(module: &'a Module, name: &str) -> Option<&'a Shape> {
    module
        .shapes
        .iter()
        .find(|s| matches!(s.kind, ShapeKind::Entry { .. }) && bare_entry_name(&s.id) == name)
}

fn entry_operations(entry: &Shape) -> &[Shape] {
    match &entry.kind {
        ShapeKind::Entry { operations, .. } => operations,
        _ => &[],
    }
}

/// The names an `ext impl` may use to reach the operation with this id (the
/// bare name, and the qualified `entry.op` form).
fn impl_names(op_id: &str) -> Vec<&str> {
    let local = op_id.rsplit('#').next().unwrap_or(op_id);
    match local.split_once('.') {
        Some((_, bare)) => vec![local, bare],
        None => vec![local],
    }
}

/// Whether the module binds an `ext impl` to the operation (any language).
fn has_impl(module: &Module, op_id: &str) -> bool {
    let names = impl_names(op_id);
    module
        .extensions
        .iter()
        .any(|e| e.kind == ExtKind::Impl && names.contains(&e.name.as_str()))
}

/// Whether every `ext impl` bound to the operation is raw (the raw glue
/// discriminates failures by `@errorCode`, so a raw error answer needs one).
fn impl_is_raw(module: &Module, op_id: &str) -> bool {
    let names = impl_names(op_id);
    module
        .extensions
        .iter()
        .filter(|e| e.kind == ExtKind::Impl && names.contains(&e.name.as_str()))
        .any(|e| e.raw)
}

/// The declared error of the operation whose shape has the given bare name.
pub fn declared_error_by_shape(
    op: &Shape,
    module: &Module,
    shape: &str,
) -> Option<crate::codegen::ops::DeclaredError> {
    crate::codegen::ops::declared_errors(op, module)
        .into_iter()
        .find(|e| e.shape_id.rsplit('#').next().unwrap_or(&e.shape_id) == shape)
}

/// The closed taxonomy vocabulary and the fields each category exposes to a
/// pattern (mirrors the frontend's `tono.errors` shapes).
const TAXONOMY: [(&str, &[&str]); 6] = [
    ("api", &["status", "body"]),
    ("validation", &["fields"]),
    ("decode", &["path"]),
    ("contract", &["name"]),
    ("config", &["field"]),
    ("transport", &[]),
];

const PHASE_MULTI_CALL: &str = "multi-call flows are not generated yet";

fn plan_test<'a>(module: &'a Module, test: &'a TestDecl) -> Result<PlannedTest<'a>, String> {
    let construction = match test.constructions.as_slice() {
        [one] => one,
        [] => return Err("declares no construction".into()),
        _ => {
            return Err(format!(
                "constructs more than one client; {PHASE_MULTI_CALL}"
            ))
        }
    };
    let entry = entry_shape(module, &construction.entry).ok_or_else(|| {
        format!(
            "constructs '{}', which is not an entry of the module",
            construction.entry
        )
    })?;
    validate_values(entry, construction)?;

    let call = match test.calls.as_slice() {
        [] => None,
        [one] => Some(one),
        _ => return Err(PHASE_MULTI_CALL.to_string()),
    };
    let op = match call {
        None => None,
        Some(call) => Some(resolve_call(entry, construction, call)?),
    };

    let stub = match test.stubs.as_slice() {
        [] => None,
        [one] => Some(one),
        _ => return Err(format!("declares more than one stub; {PHASE_MULTI_CALL}")),
    };
    if let Some(stub) = stub {
        let call = call.ok_or("stubs a dependency but never calls an operation")?;
        validate_stub(module, entry, construction, call, stub)?;
    }

    let (outcome, requests) = resolve_expects(module, construction, call, op, stub, test)?;

    Ok(PlannedTest {
        name: &test.name,
        construction,
        stub,
        call,
        op,
        outcome,
        requests,
        hermetic: call.is_none() || stub.is_some(),
    })
}

/// Every construction value names an entry field. An `@arg` field left
/// unpinned is not rejected: the generated test passes the type's zero value,
/// so a construction-failure expectation stays expressible.
fn validate_values(entry: &Shape, construction: &TestConstruction) -> Result<(), String> {
    let fields = match &entry.kind {
        ShapeKind::Entry { fields, .. } => fields,
        _ => return Ok(()),
    };
    for key in construction.values.keys() {
        if !fields.iter().any(|f| f.name == *key) {
            return Err(format!(
                "pins '{key}', which is not a field of entry '{}'",
                construction.entry
            ));
        }
    }
    Ok(())
}

fn resolve_call<'a>(
    entry: &'a Shape,
    construction: &TestConstruction,
    call: &TestCall,
) -> Result<&'a Shape, String> {
    if call.client != construction.binding {
        return Err(format!(
            "calls through '{}', but the test constructs '{}'",
            call.client, construction.binding
        ));
    }
    let op = entry_operations(entry)
        .iter()
        .find(|op| bare_op_name(&op.id) == call.op)
        .ok_or_else(|| {
            format!(
                "calls '{}', which is not an operation of entry '{}'",
                call.op, construction.entry
            )
        })?;
    if let Some(input) = &call.input {
        if contains_ref(input) {
            return Err(format!(
                "the call input references another binding; {PHASE_MULTI_CALL}"
            ));
        }
    }
    Ok(op)
}

/// The whole wire object a closed pattern whose every field is a plain `eq`
/// pins down. Such a pattern names each field with a literal value and refuses
/// unmentioned keys, so it is equivalent to one total comparison in wire form;
/// an open pattern or a marker leaf (`present`/`absent`) keeps the
/// field-by-field checks and returns `None`.
pub fn closed_eq_object(pattern: &ShapePattern) -> Option<Value> {
    if pattern.open {
        return None;
    }
    let mut object = serde_json::Map::new();
    for (key, leaf) in &pattern.fields {
        let FieldPattern::Pat(TestPattern::Eq(value)) = leaf else {
            return None;
        };
        object.insert(key.clone(), value.clone());
    }
    Some(Value::Object(object))
}

/// The error shapes some declared test pins whole (a closed all-`eq` error
/// pattern): their generated assertion re-encodes the decoded error for one
/// total wire comparison, so a target that would otherwise skip an error-only
/// shape's encoder must emit it. Names are the shapes' bare local names.
pub fn error_shapes_pinned_whole(module: &Module) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for test in &module.tests {
        for expect in &test.expects {
            let TestExpect::Outcome {
                pattern: TestPattern::Error(shape),
                ..
            } = expect
            else {
                continue;
            };
            if closed_eq_object(shape).is_some() {
                names.insert(shape.shape.clone());
            }
        }
    }
    names
}

/// Whether a wire-form value carries a `$ref` binding reference anywhere.
fn contains_ref(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$ref") || map.values().any(contains_ref),
        Value::Array(items) => items.iter().any(contains_ref),
        _ => false,
    }
}

fn validate_stub(
    module: &Module,
    entry: &Shape,
    construction: &TestConstruction,
    call: &TestCall,
    stub: &TestStub,
) -> Result<(), String> {
    if stub.client != construction.binding {
        return Err(format!(
            "stubs through '{}', but the test constructs '{}'",
            stub.client, construction.binding
        ));
    }
    let op = entry_operations(entry)
        .iter()
        .find(|op| bare_op_name(&op.id) == stub.op)
        .ok_or_else(|| {
            format!(
                "stubs '{}', which is not an operation of entry '{}'",
                stub.op, construction.entry
            )
        })?;
    if stub.op != call.op {
        return Err(format!(
            "stubs '{}' but calls '{}'; {PHASE_MULTI_CALL}",
            stub.op, call.op
        ));
    }
    let has_wire = crate::codegen::ops::wire_binding(op).is_some();
    match stub.dep {
        StubDep::Http if !has_wire => {
            return Err(format!(
                "stubs '{}.http', but the operation has no @http binding",
                stub.op
            ));
        }
        StubDep::Impl if has_wire => {
            return Err(format!(
                "stubs '{}.impl', but the operation is bound to @http",
                stub.op
            ));
        }
        StubDep::Impl if !has_impl(module, &op.id) => {
            return Err(format!(
                "stubs '{}.impl', but no ext impl binds the operation",
                stub.op
            ));
        }
        _ => {}
    }
    if stub.answers.is_empty() {
        return Err(format!("the stub on '{}' carries no answers", stub.op));
    }
    for answer in &stub.answers {
        match (stub.dep, answer) {
            (StubDep::Http, StubAnswer::Http(_)) => {}
            (StubDep::Http, _) => {
                return Err(format!(
                    "an answer of the http stub on '{}' is not an http response",
                    stub.op
                ));
            }
            (StubDep::Impl, StubAnswer::Http(_)) => {
                return Err(format!(
                    "an answer of the impl stub on '{}' is an http response",
                    stub.op
                ));
            }
            (StubDep::Impl, StubAnswer::Error { error }) => {
                let declared = declared_error_by_shape(op, module, &error.shape)
                    .ok_or_else(|| {
                        format!(
                            "an answer of the stub on '{}' names '{}', which is not a declared error of the operation",
                            stub.op, error.shape
                        )
                    })?;
                if impl_is_raw(module, &op.id) && declared.code.is_none() {
                    return Err(format!(
                        "an answer of the stub on '{}' simulates '{}', but the raw impl glue discriminates by @errorCode and the shape declares none",
                        stub.op, error.shape
                    ));
                }
            }
            (StubDep::Impl, _) => {}
        }
    }
    Ok(())
}

type Expects<'a> = (Option<&'a TestPattern>, Option<&'a [RequestPattern]>);

fn resolve_expects<'a>(
    module: &Module,
    construction: &TestConstruction,
    call: Option<&TestCall>,
    op: Option<&Shape>,
    stub: Option<&TestStub>,
    test: &'a TestDecl,
) -> Result<Expects<'a>, String> {
    let mut outcome = None;
    let mut requests = None;
    for expect in &test.expects {
        match expect {
            TestExpect::Outcome { subject, pattern } => {
                if outcome.is_some() {
                    return Err("carries more than one outcome expectation".into());
                }
                match call {
                    Some(call) if *subject == call.binding => {
                        validate_call_pattern(
                            module,
                            op.expect("call resolved with its op"),
                            pattern,
                        )?;
                    }
                    Some(_) => {
                        return Err(format!(
                            "expects over '{subject}', which is not the call's binding"
                        ));
                    }
                    None if *subject == construction.binding => {
                        validate_construction_pattern(pattern)?;
                    }
                    None => {
                        return Err(format!(
                            "expects over '{subject}', which is not the construction's binding"
                        ));
                    }
                }
                outcome = Some(pattern);
            }
            TestExpect::Requests {
                subject,
                requests: patterns,
            } => {
                if requests.is_some() {
                    return Err("carries more than one requests expectation".into());
                }
                let stub = stub.ok_or_else(|| {
                    format!("expects '{subject}.requests', but the test stubs nothing")
                })?;
                if stub.binding.as_deref() != Some(subject.as_str()) {
                    return Err(format!(
                        "expects '{subject}.requests', which is not the stub's binding"
                    ));
                }
                if stub.dep != StubDep::Http {
                    return Err(
                        "request expectations over an impl stub are not generated yet".into(),
                    );
                }
                for pattern in patterns {
                    validate_request_pattern(pattern)?;
                }
                requests = Some(patterns.as_slice());
            }
        }
    }
    Ok(requests_pair(outcome, requests))
}

fn requests_pair<'a>(
    outcome: Option<&'a TestPattern>,
    requests: Option<&'a [RequestPattern]>,
) -> Expects<'a> {
    (outcome, requests)
}

/// What an equality leaf must be: a `FieldPattern` that is a marker or a plain
/// wire value. A nested structural pattern would need recursive per-field
/// comparison the emitters do not produce yet, so it is refused loudly.
fn validate_field_leaf(field: &str, pattern: &FieldPattern) -> Result<(), String> {
    match pattern {
        FieldPattern::Absent { .. } | FieldPattern::Present { .. } => Ok(()),
        FieldPattern::Pat(TestPattern::Eq(_)) => Ok(()),
        FieldPattern::Pat(_) => Err(format!(
            "field '{field}' carries a nested structural pattern; \
             nested patterns are not generated yet"
        )),
    }
}

fn validate_call_pattern(module: &Module, op: &Shape, pattern: &TestPattern) -> Result<(), String> {
    let (_, output) = crate::codegen::ops::op_io(op);
    match pattern {
        TestPattern::Ok(_) => Ok(()),
        TestPattern::Eq(_) | TestPattern::Struct(_) if output.is_none() => Err(format!(
            "expects a value, but '{}' declares no output",
            bare_op_name(&op.id)
        )),
        TestPattern::Eq(_) => Ok(()),
        TestPattern::Struct(shape) => {
            for (field, leaf) in &shape.fields {
                validate_field_leaf(field, leaf)?;
            }
            Ok(())
        }
        TestPattern::Error(shape) => {
            if declared_error_by_shape(op, module, &shape.shape).is_none() {
                return Err(format!(
                    "expects error '{}', which is not a declared error of '{}'",
                    shape.shape,
                    bare_op_name(&op.id)
                ));
            }
            for (field, leaf) in &shape.fields {
                validate_field_leaf(field, leaf)?;
            }
            Ok(())
        }
        TestPattern::Taxonomy(tax) => validate_taxonomy_pattern(&tax.category, &tax.fields),
    }
}

/// A construction outcome is the client or a construction failure: only the
/// success marker and the taxonomy categories read on it.
fn validate_construction_pattern(pattern: &TestPattern) -> Result<(), String> {
    match pattern {
        TestPattern::Ok(_) => Ok(()),
        TestPattern::Taxonomy(tax) => validate_taxonomy_pattern(&tax.category, &tax.fields),
        _ => Err("a construction outcome matches ok or an errors.* category only".into()),
    }
}

fn validate_taxonomy_pattern(
    category: &str,
    fields: &std::collections::BTreeMap<String, FieldPattern>,
) -> Result<(), String> {
    let Some((_, allowed)) = TAXONOMY.iter().find(|(name, _)| *name == category) else {
        let known: Vec<&str> = TAXONOMY.iter().map(|(name, _)| *name).collect();
        return Err(format!(
            "unknown error category 'errors.{category}'; the taxonomy is {}",
            known.join(", ")
        ));
    };
    for (field, leaf) in fields {
        if !allowed.contains(&field.as_str()) {
            return Err(format!("'errors.{category}' has no field '{field}'"));
        }
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(_)) => {}
            _ => {
                return Err(format!(
                    "field '{field}' of 'errors.{category}' supports equality only"
                ));
            }
        }
    }
    Ok(())
}

/// A request pattern names the request surface (`method`/`path`) with string
/// equality plus a headers subset; the whole pattern list matches all recorded
/// requests in order with equal length, so there is no separate count assert.
fn validate_request_pattern(pattern: &RequestPattern) -> Result<(), String> {
    if !pattern.open {
        return Err("closed request patterns are not generated yet (add `..`)".into());
    }
    for (field, leaf) in &pattern.fields {
        if field != "method" && field != "path" {
            return Err(format!(
                "request field '{field}' is not generated yet (method, path and headers are)"
            ));
        }
        match leaf {
            FieldPattern::Pat(TestPattern::Eq(Value::String(_))) => {}
            _ => {
                return Err(format!(
                    "request field '{field}' supports string equality only"
                ));
            }
        }
    }
    for (header, leaf) in pattern.headers.iter().flatten() {
        match leaf {
            FieldPattern::Absent { .. } | FieldPattern::Present { .. } => {}
            FieldPattern::Pat(TestPattern::Eq(Value::String(_))) => {}
            _ => {
                return Err(format!(
                    "request header '{header}' supports string equality, presence or absence only"
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "declared_tests_tests.rs"]
mod tests;

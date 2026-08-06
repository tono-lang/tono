//! Shared declared-test (`Module.tests`) builders: the per-target emitter
//! tests feed the same declared tests through different native test emitters,
//! so the fixture spelling lives once here and each target's test file keeps
//! only its own assertions over the generated text.

use std::collections::BTreeMap;

use crate::ir::{
    Empty, ExtKind, Extension, FieldPattern, HttpAnswer, Module, RequestPattern, ShapePattern,
    StubAnswer, StubDep, TaxonomyPattern, TestCall, TestConstruction, TestDecl, TestExpect,
    TestPattern, TestStub,
};

/// An equality leaf over a wire-form value.
pub fn eq(value: serde_json::Value) -> FieldPattern {
    FieldPattern::Pat(TestPattern::Eq(value))
}

/// The presence marker leaf.
pub fn present() -> FieldPattern {
    FieldPattern::Present { present: Empty {} }
}

/// The absence marker leaf.
pub fn absent() -> FieldPattern {
    FieldPattern::Absent { absent: Empty {} }
}

/// Named fields collected into the map every structural pattern carries.
pub fn pattern_fields(fields: Vec<(&str, FieldPattern)>) -> BTreeMap<String, FieldPattern> {
    fields
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

/// An open taxonomy pattern over an error category.
pub fn taxonomy(category: &str, fields: Vec<(&str, FieldPattern)>) -> TestPattern {
    TestPattern::Taxonomy(TaxonomyPattern {
        category: category.into(),
        open: true,
        fields: pattern_fields(fields),
    })
}

/// An open request pattern: `method`/`path` string equalities plus a headers
/// subset (the only form validation admits).
pub fn request_pattern(
    fields: Vec<(&str, &str)>,
    headers: Vec<(&str, FieldPattern)>,
) -> RequestPattern {
    RequestPattern {
        open: true,
        fields: fields
            .into_iter()
            .map(|(k, v)| (k.to_string(), eq(serde_json::json!(v))))
            .collect(),
        headers: Some(
            headers
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        ),
    }
}

/// An `ext impl` extension bound for one language.
pub fn impl_extension(lang: &str, op: &str, binding: &str, raw: bool) -> Extension {
    Extension {
        name: op.into(),
        kind: ExtKind::Impl,
        signature: None,
        raw,
        bindings: [(lang.to_string(), binding.to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }
}

/// Install declared tests on a module.
pub fn with_tests(mut module: Module, tests: Vec<TestDecl>) -> Module {
    module.tests = tests;
    module
}

/// Mark every entry op as wire-bound (so an `.http` stub validates) and
/// install the declared tests.
pub fn wired(mut module: Module, tests: Vec<TestDecl>) -> Module {
    super::push_entry_op_wire(&mut module, "POST");
    with_tests(module, tests)
}

/// The per-fixture constants of one entry under declared-test exercise; the
/// builders below read them, so a target's tests carry only their assertions.
pub struct DeclaredTestBed {
    /// The entry's bare name.
    pub entry: &'static str,
    /// The operation the tests call and stub.
    pub op: &'static str,
    /// The head of a struct pattern over the op's output.
    pub output_shape: &'static str,
    /// The bare name of the op's declared error shape.
    pub error_shape: &'static str,
    /// The call's declared wire-form input (also echoed as the answer body).
    pub input: serde_json::Value,
}

/// The bed over the schema fixture (`notes#client.save_note`).
pub fn notes_bed() -> DeclaredTestBed {
    DeclaredTestBed {
        entry: "client",
        op: "save_note",
        output_shape: "note",
        error_shape: "overloaded",
        input: serde_json::json!({"id": "n1"}),
    }
}

/// The bed over the simple-entry fixture (`m#client.create_charge`).
pub fn charge_bed() -> DeclaredTestBed {
    DeclaredTestBed {
        entry: "client",
        op: "create_charge",
        output_shape: "charge",
        error_shape: "payment_declined",
        input: serde_json::json!({"id": "c1"}),
    }
}

impl DeclaredTestBed {
    /// The construction under the `c` binding, pinning the `@arg` to `"k"`.
    pub fn construction(&self) -> TestConstruction {
        TestConstruction {
            binding: "c".into(),
            entry: self.entry.into(),
            values: BTreeMap::from([("api_key".to_string(), serde_json::json!("k"))]),
        }
    }

    /// The call under the `saved` binding, passing the declared input.
    pub fn call(&self) -> TestCall {
        TestCall {
            binding: "saved".into(),
            client: "c".into(),
            op: self.op.into(),
            input: Some(self.input.clone()),
        }
    }

    /// A canned response echoing the declared input back.
    pub fn http_answer(&self, status: i64) -> StubAnswer {
        StubAnswer::Http(HttpAnswer {
            status,
            headers: BTreeMap::new(),
            body: serde_json::to_string(&self.input).expect("input encodes"),
        })
    }

    /// An `.http` stub answering one canned 200.
    pub fn http_stub(&self, binding: Option<&str>) -> TestStub {
        TestStub {
            binding: binding.map(str::to_string),
            client: "c".into(),
            op: self.op.into(),
            dep: StubDep::Http,
            answers: vec![self.http_answer(200)],
        }
    }

    /// The eq expectation over the call's outcome pinning the declared input.
    pub fn echo_expect(&self) -> TestExpect {
        TestExpect::Outcome {
            subject: "saved".into(),
            pattern: TestPattern::Eq(self.input.clone()),
        }
    }

    /// One stubbed call whose outcome must satisfy the pattern.
    pub fn outcome_test(&self, name: &str, pattern: TestPattern) -> TestDecl {
        TestDecl {
            name: name.into(),
            constructions: vec![self.construction()],
            stubs: vec![self.http_stub(None)],
            calls: vec![self.call()],
            expects: vec![TestExpect::Outcome {
                subject: "saved".into(),
                pattern,
            }],
        }
    }

    /// A structural pattern over the op's output shape.
    pub fn struct_pattern(&self, open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
        ShapePattern {
            shape: self.output_shape.into(),
            open,
            fields: pattern_fields(fields),
        }
    }

    /// A structural pattern over the op's declared error shape.
    pub fn error_pattern(&self, open: bool, fields: Vec<(&str, FieldPattern)>) -> ShapePattern {
        ShapePattern {
            shape: self.error_shape.into(),
            open,
            fields: pattern_fields(fields),
        }
    }

    /// The declared input's `id` value, reused by the struct suites.
    fn id(&self) -> serde_json::Value {
        self.input["id"].clone()
    }

    /// A bare success expectation over the stubbed call.
    pub fn ok_test(&self) -> TestDecl {
        self.outcome_test("just works", TestPattern::Ok(Empty {}))
    }

    /// An open struct pattern: one equality, one presence, one absence.
    pub fn open_struct_test(&self) -> TestDecl {
        self.outcome_test(
            "matches loosely",
            TestPattern::Struct(self.struct_pattern(
                true,
                vec![
                    ("id", eq(self.id())),
                    ("extra", present()),
                    ("missing", absent()),
                ],
            )),
        )
    }

    /// A closed struct pattern of plain equalities, which collapses into one
    /// total wire comparison.
    pub fn closed_eq_struct_test(&self) -> TestDecl {
        self.outcome_test(
            "pins the wire object",
            TestPattern::Struct(self.struct_pattern(
                false,
                vec![("id", eq(self.id())), ("tag", eq(serde_json::json!("t")))],
            )),
        )
    }

    /// A closed struct pattern with a marker, which keeps the per-field checks
    /// and rejects unmentioned keys.
    pub fn closed_marker_struct_test(&self) -> TestDecl {
        self.outcome_test(
            "pins the keys",
            TestPattern::Struct(
                self.struct_pattern(false, vec![("id", eq(self.id())), ("tag", present())]),
            ),
        )
    }

    /// Every declared-error pattern form: open with equalities (one naming no
    /// member), open presence/absence, closed with a marker, closed all-eq
    /// (the collapsing form), and bare.
    pub fn error_suite(&self) -> Vec<TestDecl> {
        let busy = || eq(serde_json::json!("busy"));
        vec![
            self.outcome_test(
                "open error fields",
                TestPattern::Error(self.error_pattern(
                    true,
                    vec![("message", busy()), ("bogus", eq(serde_json::json!("x")))],
                )),
            ),
            self.outcome_test(
                "open error present",
                TestPattern::Error(self.error_pattern(true, vec![("message", present())])),
            ),
            self.outcome_test(
                "open error absent",
                TestPattern::Error(self.error_pattern(true, vec![("message", absent())])),
            ),
            self.outcome_test(
                "closed error marker",
                TestPattern::Error(self.error_pattern(false, vec![("message", present())])),
            ),
            self.outcome_test(
                "closed error total",
                TestPattern::Error(self.error_pattern(false, vec![("message", busy())])),
            ),
            self.outcome_test(
                "bare error",
                TestPattern::Error(self.error_pattern(true, vec![])),
            ),
        ]
    }

    /// Every taxonomy category with its fields matched; the contract name is
    /// the called op's.
    pub fn taxonomy_suite(&self) -> Vec<TestDecl> {
        vec![
            self.outcome_test(
                "api tax",
                taxonomy(
                    "api",
                    vec![
                        ("status", eq(serde_json::json!(500))),
                        ("body", eq(serde_json::json!("boom"))),
                    ],
                ),
            ),
            self.outcome_test(
                "validation tax",
                taxonomy(
                    "validation",
                    vec![("fields", eq(serde_json::json!(["id"])))],
                ),
            ),
            self.outcome_test(
                "decode tax",
                taxonomy("decode", vec![("path", eq(serde_json::json!("$.id")))]),
            ),
            self.outcome_test(
                "contract tax",
                taxonomy("contract", vec![("name", eq(serde_json::json!(self.op)))]),
            ),
            self.outcome_test(
                "config tax",
                taxonomy("config", vec![("field", eq(serde_json::json!("api_key")))]),
            ),
            self.outcome_test("transport tax", taxonomy("transport", vec![])),
        ]
    }

    /// The field-bearing categories again with no fields at all, for the
    /// targets whose emitters spell a distinct no-field binding.
    pub fn taxonomy_bare_suite(&self) -> Vec<TestDecl> {
        ["api", "validation", "decode", "contract", "config"]
            .iter()
            .map(|category| {
                self.outcome_test(&format!("{category} bare"), taxonomy(category, vec![]))
            })
            .collect()
    }

    /// A named stub whose recorded request must carry one header and lack
    /// another.
    pub fn request_marker_test(&self) -> TestDecl {
        TestDecl {
            name: "traces the request".into(),
            constructions: vec![self.construction()],
            stubs: vec![self.http_stub(Some("s"))],
            calls: vec![self.call()],
            expects: vec![TestExpect::Requests {
                subject: "s".into(),
                requests: vec![request_pattern(
                    vec![],
                    vec![("X-Trace", present()), ("X-Debug", absent())],
                )],
            }],
        }
    }

    /// A two-answer stub (500 then 200) under a named binding, the given
    /// outcome, and the same bearer-token request pattern expected twice (the
    /// length check doubles as the retry-count assert).
    pub fn retry_request_test(&self, path: &str, outcome: TestPattern) -> TestDecl {
        let request = request_pattern(
            vec![("method", "POST"), ("path", path)],
            vec![("authorization", eq(serde_json::json!("Bearer k")))],
        );
        TestDecl {
            name: "sends the token twice".into(),
            constructions: vec![self.construction()],
            stubs: vec![TestStub {
                binding: Some("s".into()),
                client: "c".into(),
                op: self.op.into(),
                dep: StubDep::Http,
                // A sequence: the second call consumes the second response,
                // and any further call repeats the last.
                answers: vec![self.http_answer(500), self.http_answer(200)],
            }],
            calls: vec![self.call()],
            expects: vec![
                TestExpect::Outcome {
                    subject: "saved".into(),
                    pattern: outcome,
                },
                TestExpect::Requests {
                    subject: "s".into(),
                    requests: vec![request.clone(), request],
                },
            ],
        }
    }

    /// One hermetic (impl-stubbed) echo test and one live echo test.
    pub fn impl_echo_tests(&self) -> Vec<TestDecl> {
        let hermetic = TestDecl {
            name: "stores it".into(),
            constructions: vec![self.construction()],
            stubs: vec![TestStub {
                binding: None,
                client: "c".into(),
                op: self.op.into(),
                dep: StubDep::Impl,
                answers: vec![StubAnswer::Value {
                    value: self.input.clone(),
                }],
            }],
            calls: vec![self.call()],
            expects: vec![self.echo_expect()],
        };
        let live = TestDecl {
            name: "hits the real store".into(),
            stubs: vec![],
            ..hermetic.clone()
        };
        vec![hermetic, live]
    }
}

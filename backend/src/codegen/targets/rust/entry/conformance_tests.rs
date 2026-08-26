//! The declared-test half of the Rust entry tests: the constructor's
//! transport seam and the native test files the declared tests generate.
//! Split from `tests.rs` so each file stays within the repo size gate.

use std::collections::BTreeMap;

use super::tests::simple_entry_module;
use super::*;
use crate::codegen::targets::rust::rust_casing;
use crate::codegen::test_support::{bare_entry_field, push_entry_field, rendered};
use crate::ir::{
    FieldPattern, HttpAnswer, RequestPattern, Source, StubAnswer, StubDep, TaxonomyPattern,
    TestCall, TestConstruction, TestDecl, TestExpect, TestPattern, TestStub,
};

fn construction() -> TestConstruction {
    TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([
            ("api_key".to_string(), serde_json::json!("k")),
            ("client_name".to_string(), serde_json::json!("demo")),
        ]),
    }
}

fn call() -> TestCall {
    TestCall {
        binding: "got".into(),
        client: "c".into(),
        op: "create_charge".into(),
        input: Some(serde_json::json!({"id": "c1"})),
    }
}

fn eq(value: serde_json::Value) -> FieldPattern {
    FieldPattern::Pat(TestPattern::Eq(value))
}

fn http_stub(answers: Vec<HttpAnswer>) -> TestStub {
    TestStub {
        binding: Some("s".into()),
        client: "c".into(),
        op: "create_charge".into(),
        dep: StubDep::Http,
        answers: answers.into_iter().map(StubAnswer::Http).collect(),
    }
}

fn answer(status: i64, body: &str) -> HttpAnswer {
    HttpAnswer {
        status,
        headers: BTreeMap::new(),
        body: body.into(),
    }
}

/// The declared tests of the fixture's `create_charge`: a stubbed ok test
/// with request expectations, a stubbed api-error test, and a live test.
fn create_charge_tests() -> Vec<TestDecl> {
    let request = RequestPattern {
        open: true,
        fields: BTreeMap::from([
            ("method".to_string(), eq(serde_json::json!("POST"))),
            ("path".to_string(), eq(serde_json::json!("/charges"))),
        ]),
        headers: Some(BTreeMap::from([(
            "authorization".to_string(),
            eq(serde_json::json!("Bearer k")),
        )])),
    };
    vec![
        TestDecl {
            name: "answers the charge".into(),
            constructions: vec![construction()],
            stubs: vec![http_stub(vec![answer(200, "{\"id\":\"c1\"}")])],
            extern_stubs: vec![],
            calls: vec![call()],
            expects: vec![
                TestExpect::Outcome {
                    subject: "got".into(),
                    pattern: TestPattern::Eq(serde_json::json!({"id": "c1"})),
                },
                TestExpect::Requests {
                    subject: "s".into(),
                    requests: vec![request],
                },
            ],
        },
        TestDecl {
            name: "surfaces the failure".into(),
            constructions: vec![construction()],
            stubs: vec![http_stub(vec![
                answer(500, "boom"),
                answer(502, "still down"),
            ])],
            extern_stubs: vec![],
            calls: vec![call()],
            expects: vec![TestExpect::Outcome {
                subject: "got".into(),
                pattern: TestPattern::Taxonomy(TaxonomyPattern {
                    category: "api".into(),
                    open: true,
                    fields: BTreeMap::from([("status".to_string(), eq(serde_json::json!(502)))]),
                }),
            }],
        },
        TestDecl {
            name: "round trips".into(),
            constructions: vec![construction()],
            stubs: vec![],
            extern_stubs: vec![],
            calls: vec![call()],
            expects: vec![TestExpect::Outcome {
                subject: "got".into(),
                pattern: TestPattern::Eq(serde_json::json!({"id": "c1"})),
            }],
        },
    ]
}

fn emitted_text(module: &Module) -> String {
    let emission = emit(module, &rust_casing());
    let mut decls = emission.shared;
    for (_, entry_decls) in emission.per_entry {
        decls.extend(entry_decls);
    }
    rendered(&decls, &RustRules::default())
}

#[test]
fn declared_tests_swap_the_constructor_for_the_transport_seam_variant() {
    let mut module = simple_entry_module();
    module.tests = create_charge_tests();
    let text = emitted_text(&module);
    // Production keeps its own transport seam for a hand-written same-crate
    // test (the cross-runtime parity harness among them): `new` delegates
    // to `new_with_transport(None, ..)`, so a real caller sees no
    // difference. A generated declared test never reaches this seam at all
    // (see below): it composes the steps directly with its own fake.
    assert!(text.contains(
        "pub fn new(api_key: String) -> Result<Self, TonoError> {\n        Self::new_with_transport(None, api_key)\n    }"
    ));
    assert!(text.contains(
        "pub(crate) fn new_with_transport(transport: Option<HttpTransport>, api_key: String) -> Result<Self, TonoError> {\n        let mut s = Self::new_settings(api_key)?;\n        if let Some(t) = transport {\n            s.transport = Some(t);\n            #[cfg(feature = \"reqwest\")]\n            {\n                s.client = None;\n            }\n        }\n        Self::new_client(s)\n    }"
    ));
    assert!(text
        .contains("pub(crate) fn new_settings(api_key: String) -> Result<Settings, TonoError> {"));
    assert!(text.contains("pub(crate) fn new_client(s: Settings) -> Result<Self, TonoError> {"));
    // A generated test builds the client itself: settings, then the
    // transport injected directly, then the client, with no branch in `new`
    // deciding between production and test.
    let files = super::vector_tests::test_files(&module, &rust_casing());
    let hermetic = rendered(&files[0].file.decls, &RustRules::default());
    assert!(hermetic.contains("let mut s = Client::new_settings(\"k\".to_string())?;"));
    assert!(hermetic.contains("s.transport = Some(transport);"));
    assert!(hermetic.contains("s.client = None;"));
}

#[test]
fn a_with_bearing_entry_routes_build_through_the_seam() {
    let mut module = simple_entry_module();
    module.tests = create_charge_tests();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "timeout_ms",
            Tref::Prim(Prim::I32),
            vec![Source::With, Source::Default(serde_json::json!(30))],
        ),
    );
    let text = emitted_text(&module);
    // `build` keeps its own transport seam for a hand-written same-crate
    // test, mirroring `new`'s own delegation.
    assert!(text.contains(
        "pub fn build(self) -> Result<Client, TonoError> {\n        self.build_with_transport(None)\n    }"
    ));
    assert!(text.contains(
        "pub(crate) fn build_with_transport(self, transport: Option<HttpTransport>) -> Result<Client, TonoError> {\n        let ClientBuilder { api_key, timeout_ms } = self;\n        let mut s = Client::new_settings(api_key, timeout_ms)?;\n        if let Some(t) = transport {\n            s.transport = Some(t);\n            #[cfg(feature = \"reqwest\")]\n            {\n                s.client = None;\n            }\n        }\n        Client::new_client(s)\n    }"
    ));
    assert!(text.contains(
        "pub(crate) fn new_settings(api_key: String, timeout_ms: Option<i32>) -> Result<Settings, TonoError> {"
    ));
}

#[test]
fn declared_tests_generate_a_hermetic_and_a_live_rust_test_file() {
    let mut module = simple_entry_module();
    module.tests = create_charge_tests();
    let files = super::vector_tests::test_files(&module, &rust_casing());
    assert_eq!(files.len(), 2);

    assert_eq!(files[0].group.tests_of(), Some(("client", false)));
    let hermetic = rendered(&files[0].file.decls, &RustRules::default());
    // The file is a test module of the crate: it reaches the client and the
    // wire types through globs, and serializes env writes on a lock.
    assert!(hermetic.contains("use crate::m::types::*;"));
    assert!(hermetic.contains("use crate::m::client::*;"));
    assert!(hermetic.contains("static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());"));
    assert!(hermetic.contains("#[tokio::test]\nasync fn create_charge_answers_the_charge() {"));
    // A pinned @env member is set, and construction goes through the seam
    // with the @arg value from the pinned construction values.
    assert!(hermetic.contains("std::env::set_var(\"CLIENT_NAME\", \"demo\");"));
    assert!(hermetic.contains(
        "let c = (|| -> Result<Client, TonoError> {\n        let mut s = Client::new_settings(\"k\".to_string())?;\n        s.transport = Some(transport);\n        #[cfg(feature = \"reqwest\")]\n        {\n            s.client = None;\n        }\n        Ok::<_, TonoError>(Client::new_client(s)?)\n    })().expect(\"construct client\");"
    ));
    // The stubbed transport records every canonical request and answers its
    // single canned response directly (no sequence index); the input decodes
    // off the declared wire text.
    assert!(hermetic
        .contains("recorded.lock().unwrap_or_else(|e| e.into_inner()).push(req);\n                Ok(HttpResponse {"));
    assert!(hermetic.contains("status: 200,"));
    assert!(hermetic
        .contains("let input: Charge = serde_json::from_str(r#\"{\"id\":\"c1\"}\"#).expect(\"decode declared input\");"));
    // The wire spelling is what is compared, never the language spelling.
    assert!(hermetic.contains("serde_json::to_value(&out)"));
    // The whole request-pattern list matches all recorded requests with equal
    // length; the path strips scheme/host inline and the headers compare
    // through one lowercased copy per request (no helper functions).
    assert!(hermetic.contains("assert_eq!(seen.len(), 1, \"recorded requests\");"));
    assert!(hermetic.contains("assert_eq!(req0.method, \"POST\", \"request 0 method\");"));
    assert!(hermetic.contains(
        "let rest0 = req0.url.split_once(\"://\").map_or(req0.url.as_str(), |(_, rest)| rest);"
    ));
    assert!(hermetic.contains("let path0 = rest0.find('/').map_or(\"/\", |at| &rest0[at..]);"));
    assert!(hermetic.contains("assert_eq!(path0, \"/charges\", \"request 0 path\");"));
    assert!(hermetic.contains("let lower0: std::collections::HashMap<String, String> = req0"));
    assert!(hermetic.contains(".map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))"));
    assert!(hermetic.contains(
        "assert_eq!(lower0.get(\"authorization\").map(String::as_str).unwrap_or(\"\"), \"Bearer k\", \"request 0 header authorization\");"
    ));
    assert!(!hermetic.contains("fn request_path"));
    assert!(!hermetic.contains("fn header_value"));
    // An answer sequence lowers to an index over the recorded calls, the last
    // response repeating; the taxonomy pattern matches the api category.
    assert!(hermetic.contains("async fn create_charge_surfaces_the_failure() {"));
    assert!(hermetic.contains("Ok(match i {"));
    assert!(hermetic.contains("Err(TonoError::Api(APIFailure::Undeclared(e))) => e,"));
    assert!(hermetic.contains("assert_eq!(api.status, 502, \"api status\");"));

    // The live test constructs off the ambient environment through the public
    // constructor, tagged out of a default run by `#[ignore]` (the `_live`
    // suffix keeps the names apart from the hermetic file's).
    assert_eq!(files[1].group.tests_of(), Some(("client", true)));
    let live = rendered(&files[1].file.decls, &RustRules::default());
    assert!(
        live.contains("#[ignore = \"runs against the live endpoint; pass --ignored to include\"]")
    );
    assert!(live.contains("async fn create_charge_round_trips_live() {"));
    assert!(live.contains("let c = Client::new(\"k\".to_string()).expect(\"construct client\");"));
    assert!(!live.contains("ENV_LOCK"));
    assert!(!live.contains("std::env::set_var"));
}

#[test]
fn impl_stubbed_tests_are_skipped_for_rust() {
    let mut module = simple_entry_module();
    // Rebind the op as bespoke: strip its wire binding and bind an impl.
    for shape in &mut module.shapes {
        if let crate::ir::ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                if let crate::ir::ShapeKind::Operation { wire, .. } = &mut op.kind {
                    *wire = None;
                }
            }
        }
    }
    module.extensions = vec![crate::ir::Extension {
        name: "create_charge".into(),
        kind: crate::ir::ExtKind::Impl,
        signature: None,
        raw: false,
        bindings: BTreeMap::from([("rust".to_string(), "ext/rust/charge.rs#charge".to_string())]),
        conformance: None,
    }];
    let impl_test = TestDecl {
        name: "stores it".into(),
        constructions: vec![construction()],
        stubs: vec![TestStub {
            binding: None,
            client: "c".into(),
            op: "create_charge".into(),
            dep: StubDep::Impl,
            answers: vec![StubAnswer::Value {
                value: serde_json::json!({"id": "c1"}),
            }],
        }],
        extern_stubs: vec![],
        calls: vec![call()],
        expects: vec![],
    };
    let live_test = TestDecl {
        name: "round trips".into(),
        stubs: vec![],
        ..impl_test.clone()
    };
    // An impl-stubbed test beside a live one: only the live file is emitted.
    module.tests = vec![impl_test.clone(), live_test];
    let files = super::vector_tests::test_files(&module, &rust_casing());
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].group.tests_of(), Some(("client", true)));
    // Only impl-stubbed tests: nothing is emitted at all.
    module.tests = vec![impl_test];
    assert!(super::vector_tests::test_files(&module, &rust_casing()).is_empty());
}

#[test]
fn a_pinned_list_is_a_vec_literal() {
    let mut module = simple_entry_module();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "samples",
            Tref::List(Box::new(Tref::Prim(Prim::Float))),
            vec![Source::Arg],
        ),
    );
    push_entry_field(
        &mut module,
        bare_entry_field(
            "names",
            Tref::List(Box::new(Tref::Prim(Prim::String))),
            vec![Source::Arg],
        ),
    );
    let mut construction = construction();
    construction
        .values
        .insert("samples".into(), serde_json::json!([1.0, 2.0, 3.0]));
    let mut test = create_charge_tests().remove(0);
    test.constructions = vec![construction];
    module.tests = vec![test];
    let files = super::vector_tests::test_files(&module, &rust_casing());
    let hermetic = rendered(&files[0].file.decls, &RustRules::default());
    // The pinned list is a `vec!` of typed literals; the unpinned `names`
    // gets the empty one.
    assert!(
        hermetic.contains(
            "let mut s = Client::new_settings(\"k\".to_string(), vec![1f64, 2f64, 3f64], vec![])?;"
        ),
        "{hermetic}"
    );
}

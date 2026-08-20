//! Coverage for the extern-stub fakes a hermetic declared test needs
//! ([`super`]): the seam constructor's per-field override arguments, the
//! fake handle type a foreign-handle stub builds, and the canned answer a
//! fake method body renders. The shared appendix fixture already wires a
//! plain free-fn stub and a stubbed handle method through the real
//! validation pipeline, so most scenarios here reuse it as-is; the branches
//! that pipeline never reaches (no stub at all, an unstubbed handle method,
//! the non-`Value` canned answers) build their own minimal `PlannedTest` or
//! call the private renderers directly instead.

use std::collections::BTreeMap;

use crate::codegen::declared_tests::{self, PlannedTest};
use crate::codegen::entries::plan;
use crate::codegen::targets::go::entry::ext_fixtures;
use crate::codegen::targets::go::entry::vector_tests::vector_extern::{
    extern_override_args, fake_method_body, handle_fake_decl,
};
use crate::codegen::targets::go::entry::vector_tests::{TestCtx, BINDING_LANGS};
use crate::codegen::targets::go::entry::{names, Names};
use crate::codegen::targets::go::types::go_casing;
use crate::codegen::targets::go::GoRules;
use crate::codegen::test_support::rendered;
use crate::ir::{
    AnswerError, Empty, ExtLib, ExternLang, ExternStub, ExternStubTarget, HttpAnswer, LangPath,
    OpaqueType, Prim, StubAnswer, TestConstruction, Tref,
};

fn appendix_ctx<'a>(
    module: &'a crate::ir::Module,
    entries: &'a [crate::codegen::entries::EntryModel<'a>],
    multi: bool,
    n: &'a Names,
    config: &'a crate::codegen::casing::CasingConfig,
    test: &'a PlannedTest<'a>,
) -> TestCtx<'a> {
    TestCtx {
        entry: &entries[0],
        n,
        module,
        config,
        multi,
        test,
    }
}

/// The shared appendix fixture, planned into its entry setup: every test
/// below starts here, since the fixture already wires a plain free-fn stub
/// and a stubbed handle method through the real validation pipeline.
fn appendix_setup(
    module: &crate::ir::Module,
) -> (
    Vec<crate::codegen::entries::EntryModel<'_>>,
    bool,
    Names,
    crate::codegen::casing::CasingConfig,
) {
    let (entries, multi, _bound) =
        plan::entry_setup(module, &BINDING_LANGS).expect("the fixture declares an entry");
    let n = names(&entries[0], multi);
    let config = go_casing();
    (entries, multi, n, config)
}

/// [`appendix_setup`] plus the fixture's own declared tests, planned: the
/// scenarios that exercise a specific declared test (a stubbed handle
/// method, an unstubbed one) index into the returned `Vec` instead of
/// building a `PlannedTest` by hand.
fn appendix_setup_with_tests(
    module: &crate::ir::Module,
) -> (
    Vec<crate::codegen::entries::EntryModel<'_>>,
    bool,
    Names,
    crate::codegen::casing::CasingConfig,
    Vec<declared_tests::EntryTests<'_>>,
) {
    let (entries, multi, n, config) = appendix_setup(module);
    let planned = declared_tests::entry_tests(module).expect("the declared tests validate");
    (entries, multi, n, config, planned)
}

#[test]
fn a_field_with_no_matching_stub_pushes_a_nil_override_for_every_field() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config) = appendix_setup(&module);
    let construction = TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([("region".to_string(), serde_json::json!("us-east"))]),
    };
    let test = PlannedTest {
        name: "no stubs at all",
        construction: &construction,
        stub: None,
        extern_stubs: vec![],
        call: None,
        op: None,
        outcome: None,
        requests: None,
        hermetic: true,
    };
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &test);
    let mut refs = Vec::new();
    let mut extra_decls = Vec::new();
    let (pre, args) = extern_override_args(&ctx, &mut refs, &mut extra_decls);
    assert_eq!(args, vec!["nil".to_string(), "nil".to_string()]);
    assert!(pre.is_empty());
    assert!(extra_decls.is_empty());
}

#[test]
fn a_plain_free_fn_stub_decodes_a_literal_into_an_override_var() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config) = appendix_setup(&module);
    let construction = TestConstruction {
        binding: "c".into(),
        entry: "client".into(),
        values: BTreeMap::from([("region".to_string(), serde_json::json!("us-east"))]),
    };
    let config_stub = ExternStub {
        binding: None,
        target: ExternStubTarget::Free {
            lib: "companyconfig".into(),
            fn_: "load".into(),
        },
        answers: vec![StubAnswer::Value {
            value: serde_json::json!({"endpoint": "https://api.example.com", "token": "s3cr3t"}),
        }],
    };
    let test = PlannedTest {
        name: "only the plain field is stubbed",
        construction: &construction,
        stub: None,
        extern_stubs: vec![&config_stub],
        call: None,
        op: None,
        outcome: None,
        requests: None,
        hermetic: true,
    };
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &test);
    let mut refs = Vec::new();
    let mut extra_decls = Vec::new();
    let (pre, args) = extern_override_args(&ctx, &mut refs, &mut extra_decls);
    assert!(pre.contains("var configOverrideVal"));
    assert!(pre.contains("json.Unmarshal([]byte(`{\"endpoint\":\"https://api.example.com\",\"token\":\"s3cr3t\"}`), &configOverrideVal)"));
    assert_eq!(args[0], "&configOverrideVal");
    assert_eq!(args[1], "nil");
    assert!(extra_decls.is_empty());
}

#[test]
fn a_foreign_handle_stub_builds_a_fake_and_an_unstubbed_method_panics() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[0]);
    let mut refs = Vec::new();
    let mut extra_decls = Vec::new();
    let (_pre, args) = extern_override_args(&ctx, &mut refs, &mut extra_decls);
    assert_eq!(args.len(), 2);
    assert!(args[1].starts_with('&'));
    assert!(args[1].ends_with("Fake{}"));
    assert_eq!(extra_decls.len(), 1);
    let text = rendered(&extra_decls, &GoRules::default());
    assert!(text.contains("struct{}"));
    assert!(text.contains("no stub for this call in test"));
}

#[test]
fn a_stubbed_handle_method_renders_the_canned_value_answer() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[1]);
    let mut refs = Vec::new();
    let mut extra_decls = Vec::new();
    let (_pre, _args) = extern_override_args(&ctx, &mut refs, &mut extra_decls);
    let text = rendered(&extra_decls, &GoRules::default());
    assert!(text.contains("json.Unmarshal"));
    assert!(text.contains("return out, nil"));
    assert!(!text.contains("no stub for this call"));
}

/// A method language the Go emitter never wires (the fixture only declares
/// `go`) skips the fake method entirely: the interface it stands in for is
/// itself Go-only, so a non-`go` method never reaches the generated file.
#[test]
fn a_method_without_a_go_binding_is_skipped_in_the_fake() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[0]);
    let lib = ExtLib {
        name: "companybus".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "tono-ext-fixture/companybus".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![],
    };
    let handle = OpaqueType {
        name: "publisher".into(),
        interface: false,
        instance: None,
        methods: vec![crate::ir::ExternDecl {
            name: "send".into(),
            params: vec![],
            r#return: Tref::Prim(Prim::Bool),
            langs: vec![ExternLang {
                lang: "rust".into(),
                symbol: "send".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: false,
                is_new: false,
            }],
        }],
    };
    let (_ident, decl) = handle_fake_decl(&ctx, &lib, &handle);
    let text = rendered(std::slice::from_ref(&decl), &GoRules::default());
    assert!(text.contains("struct{}"));
    assert!(!text.contains("func ("));
}

/// A `ctx`-marked method's fake takes `ctx context.Context` first, exactly
/// as the interface it must satisfy declares it (the same signature the
/// real adapter renders); a fake without it would not compile against the
/// interface, which is what a field sourced from such a method at
/// construction time first exposed.
#[test]
fn a_ctx_marked_method_fake_takes_the_context_first() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[0]);
    let lib = ExtLib {
        name: "companybus".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "tono-ext-fixture/companybus".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![],
    };
    let handle = OpaqueType {
        name: "publisher".into(),
        interface: false,
        instance: None,
        methods: vec![crate::ir::ExternDecl {
            name: "send".into(),
            params: vec![crate::ir::ExternParam {
                variadic: false,
                name: "topic".into(),
                r#type: Tref::Prim(Prim::String),
            }],
            r#return: Tref::Prim(Prim::Bool),
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Send".into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
                sync: false,
                infallible: false,
                ctx: true,
                is_new: false,
            }],
        }],
    };
    let (_ident, decl) = handle_fake_decl(&ctx, &lib, &handle);
    let text = rendered(std::slice::from_ref(&decl), &GoRules::default());
    assert!(
        text.contains("Send(ctx context.Context, topic string) (bool, error) {"),
        "{text}"
    );
}

#[test]
fn fake_method_body_renders_the_declared_error_answer() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[1]);
    let ret = Tref::Ref {
        id: "m#ack".into(),
        args: vec![],
    };
    let answer = StubAnswer::Error {
        error: AnswerError {
            shape: "overloaded".into(),
            data: serde_json::json!({"message": "busy"}),
        },
    };
    let mut refs = Vec::new();
    let body = fake_method_body(&ctx, &ret, &answer, &mut refs);
    assert!(body.contains("var zero"));
    assert!(body.contains("return zero, &Overloaded{Message: \"busy\", }"));
}

/// A construction-only test (no op, `planned[0].tests[0]`) can still stub
/// a handle method reached at construction with a declared error: the
/// error shape resolves by its local name across the module, not through a
/// called op's own declared errors, so the fake renders instead of the
/// generator dying on a missing op.
#[test]
fn fake_method_body_renders_the_declared_error_answer_in_a_construction_only_test() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[0]);
    assert!(
        ctx.test.op.is_none(),
        "the fixture's first test is construction-only"
    );
    let ret = Tref::Ref {
        id: "m#ack".into(),
        args: vec![],
    };
    let answer = StubAnswer::Error {
        error: AnswerError {
            shape: "overloaded".into(),
            data: serde_json::json!({"message": "busy"}),
        },
    };
    let mut refs = Vec::new();
    let body = fake_method_body(&ctx, &ret, &answer, &mut refs);
    assert!(body.contains("return zero, &Overloaded{Message: \"busy\", }"));
}

#[test]
fn fake_method_body_renders_the_contract_dead_letter_answer() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[1]);
    let ret = Tref::Ref {
        id: "m#ack".into(),
        args: vec![],
    };
    let answer = StubAnswer::Contract { contract: Empty {} };
    let mut refs = Vec::new();
    let body = fake_method_body(&ctx, &ret, &answer, &mut refs);
    assert!(body.contains("errors.New(\"simulated bespoke failure\")"));
}

#[test]
fn fake_method_body_renders_the_defensive_http_dead_branch() {
    let module = ext_fixtures::reference_example_module();
    let (entries, multi, n, config, planned) = appendix_setup_with_tests(&module);
    let ctx = appendix_ctx(&module, &entries, multi, &n, &config, &planned[0].tests[1]);
    let ret = Tref::Ref {
        id: "m#ack".into(),
        args: vec![],
    };
    // Rejected by validation on the real pipeline: an http answer never
    // reaches a handle method stub. Reached here directly to pin the
    // defensive branch, which still must render something, not panic.
    let answer = StubAnswer::Http(HttpAnswer {
        status: 200,
        headers: BTreeMap::new(),
        body: String::new(),
    });
    let mut refs = Vec::new();
    let body = fake_method_body(&ctx, &ret, &answer, &mut refs);
    assert!(body.contains("return zero, nil"));
}

use super::*;
use crate::codegen::targets::rust::rust_casing;
use crate::codegen::test_support::{
    bare_entry_field, error_shape, member, push_entry_field, structure,
};
use crate::ir::{EnvName, ExtKind, Extension, Source, WireBinding, WireValue};

fn text(module: &Module) -> String {
    entry_text(module, &rust_casing())
}

fn charge_shape() -> Shape {
    structure(
        "m#charge",
        vec![member("id", Tref::Prim(Prim::String), true)],
    )
}

/// A module named `m` holding only the given shapes.
fn module_of(shapes: Vec<Shape>) -> Module {
    Module {
        name: "m".into(),
        shapes,
        operations: vec![],
        extensions: vec![],
        tests: vec![],
    }
}

/// Pushes an operation onto `m#client` with no protocol binding, so it only
/// resolves through a bound `ext impl` (the bespoke-impl test fixtures share
/// this shape; only the local name/input/output/errors vary between them).
fn push_client_op(
    module: &mut Module,
    local: &str,
    input: Option<Tref>,
    output: Option<Tref>,
    errors: Vec<Tref>,
) {
    let shape = module
        .shapes
        .iter_mut()
        .find(|s| s.id == "m#client")
        .expect("m#client entry shape");
    let ShapeKind::Entry { operations, .. } = &mut shape.kind else {
        panic!("m#client is not an entry shape");
    };
    operations.push(Shape {
        id: format!("m#client.{local}"),
        kind: ShapeKind::Operation {
            input,
            input_name: None,
            output,
            errors,
            wire: None,
        },
        traits: vec![],
    });
}

/// Binds an `ext impl` extension for the Rust target only.
fn push_impl_extension(module: &mut Module, name: &str, raw: bool, binding: &str) {
    module.extensions.push(Extension {
        name: name.into(),
        kind: ExtKind::Impl,
        signature: None,
        raw,
        bindings: [("rust".to_string(), binding.to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    });
}

/// Binds a lifecycle hook extension (`client_init`, `before_request`,
/// `after_response`) for the Rust target only.
fn push_hook_extension(module: &mut Module, slot: &str, binding: &str) {
    module.extensions.push(Extension {
        name: slot.into(),
        kind: ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [("rust".to_string(), binding.to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    });
}

/// A single-entry module: `@arg api_key`, `@env`/`@default` `client_name`
/// and `endpoint`, one `@http` op declaring a retryable coded error. Shared
/// with the conformance-vector tests (`conformance_tests`), which layer
/// vectors on it.
pub(super) fn simple_entry_module() -> Module {
    let api_key = bare_entry_field("api_key", Tref::Prim(Prim::String), vec![Source::Arg]);
    let client_name = bare_entry_field(
        "client_name",
        Tref::Prim(Prim::String),
        vec![
            Source::Env(EnvName::Name("CLIENT_NAME".into())),
            Source::Default(serde_json::json!("demo")),
        ],
    );
    let endpoint = bare_entry_field(
        "endpoint",
        Tref::Prim(Prim::String),
        vec![
            Source::Env(EnvName::Name("ENDPOINT".into())),
            Source::Default(serde_json::json!("https://api.test")),
        ],
    );
    let op = Shape {
        id: "m#client.create_charge".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Ref {
                id: "m#charge".into(),
                args: vec![],
            }),
            output: Some(Tref::Ref {
                id: "m#charge".into(),
                args: vec![],
            }),
            errors: vec![Tref::Ref {
                id: "m#payment_declined".into(),
                args: vec![],
            }],
            wire: Some(Box::new(WireBinding {
                method: "POST".into(),
                uri: WireValue::Template(vec![crate::ir::TemplatePart::Lit("/charges".into())]),
                body: Some(WireValue::Param(vec![])),
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: Some(WireValue::Field(vec!["endpoint".into()])),
                request_headers: vec![(
                    vec![crate::ir::TemplatePart::Lit("X-Client".into())],
                    WireValue::Field(vec!["client_name".into()]),
                )],
                query: vec![],
                timeout: None,
                retry: None,
            })),
        },
        traits: vec![Trait {
            id: "http".into(),
            value: serde_json::json!({"method": "POST", "path": "/charges"}),
        }],
    };
    module_of(vec![
        charge_shape(),
        error_shape(
            "m#payment_declined",
            vec![],
            402,
            Some("payment_declined"),
            true,
        ),
        Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![api_key, client_name, endpoint],
                operations: vec![op],
            },
            traits: vec![],
        },
    ])
}

#[test]
fn a_module_without_entries_emits_nothing() {
    let module = module_of(vec![]);
    let emission = emit(&module, &rust_casing());
    assert!(emission.shared.is_empty());
    assert!(emission.per_entry.is_empty());
}

#[test]
fn a_single_entry_with_arg_env_default_and_an_http_op_builds() {
    let module = simple_entry_module();
    let out = text(&module);
    assert!(out.contains("pub struct Client {"));
    assert!(out.contains("pub(crate) struct Settings {"));
    assert!(out.contains("pub api_key: String,"));
    assert!(out.contains("pub client_name: String,"));
    // No @with fields: a plain constructor, not a builder.
    assert!(out.contains("pub fn new(api_key: String) -> Result<Self, TonoError> {"));
    assert!(!out.contains("struct ClientBuilder"));
    assert!(out.contains("s.api_key = api_key;"));
    assert!(out.contains("read_env(\"CLIENT_NAME\")"));
    // The op is async (it carries a protocol binding) and its transport is
    // inline: the URL reads the typed endpoint field, the declared header
    // reads its typed field, the whole-body input serializes directly, and a
    // send failure maps onto the Transport category, with no descriptor and
    // no runtime crate anywhere.
    assert!(out.contains(
        "pub async fn create_charge(&self, input: Charge) -> Result<Charge, TonoError> {"
    ));
    assert!(out.contains("let url = format!(\"{}/charges\", self.settings.endpoint);"));
    assert!(
        out.contains("set_header(&mut headers, \"X-Client\", self.settings.client_name.clone());")
    );
    // A failed input serialization is a Config problem with the supplied
    // value, never the response-facing Decode category.
    assert!(out.contains(
        "let body = Some(serde_json::to_string(&input).map_err(|e| TonoError::Config(ConfigError { message: format!(\"input serialization: {e}\") }))?);"
    ));
    assert!(out.contains("match http_send(&self.options, request).await {"));
    assert!(
        out.contains("Err(cause) => return Err(TonoError::Transport(TransportError { cause })),")
    );
    assert!(
        out.contains("pub fn decode_create_charge_error(status: u16, body: &str) -> TonoError {")
    );
    assert!(!out.contains("tono_http_runtime"));
    assert!(!out.contains("WireDescriptor"));
}

#[test]
fn a_with_bearing_entry_gets_a_builder() {
    let mut module = simple_entry_module();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "timeout_ms",
            Tref::Prim(Prim::I32),
            vec![Source::With, Source::Default(serde_json::json!(30))],
        ),
    );
    let out = text(&module);
    assert!(out.contains("pub struct ClientBuilder {"));
    assert!(out.contains("pub fn with_timeout_ms(mut self, v: i32) -> Self {"));
    assert!(out.contains("pub fn builder(api_key: String) -> ClientBuilder {"));
    assert!(out.contains("pub fn build(self) -> Result<Client, TonoError> {"));
    // The @arg value is read off the builder's own field within `build`.
    assert!(out.contains("s.api_key = self.api_key;"));
}

#[test]
fn declared_constraints_on_guaranteed_with_and_numeric_fields_all_guard_on_presence() {
    let mut module = simple_entry_module();
    // A guaranteed field (@arg): the constraint check runs unconditionally.
    for shape in &mut module.shapes {
        if shape.id == "m#client" {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                fields[0].constraints.push(crate::ir::Constraint::Length {
                    min: Some(1),
                    max: None,
                });
            }
        }
    }
    // A non-guaranteed @with-only string field: the constraint guards on the
    // resolved value being non-empty.
    push_entry_field(
        &mut module,
        crate::ir::EntryField {
            constraints: vec![crate::ir::Constraint::Length {
                min: Some(1),
                max: None,
            }],
            ..bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::With])
        },
    );
    // A non-guaranteed @with-only numeric field: the constraint guards on the
    // chain having resolved at all.
    push_entry_field(
        &mut module,
        crate::ir::EntryField {
            constraints: vec![crate::ir::Constraint::Range {
                min: Some(0.0),
                max: None,
                excl_min: false,
                excl_max: false,
            }],
            ..bare_entry_field("weight", Tref::Prim(Prim::I32), vec![Source::With])
        },
    );
    // A non-guaranteed @with-only bytes field: the constraint guards on the
    // resolved value being non-empty, and its length measures in bytes.
    push_entry_field(
        &mut module,
        crate::ir::EntryField {
            constraints: vec![crate::ir::Constraint::Length {
                min: Some(1),
                max: None,
            }],
            ..bare_entry_field("receipt", Tref::Prim(Prim::Bytes), vec![Source::With])
        },
    );
    // A wide (i64) numeric field: the range literal carries the target's
    // wide-integer suffix (empty in Rust, since i64 is native).
    push_entry_field(
        &mut module,
        crate::ir::EntryField {
            constraints: vec![crate::ir::Constraint::Range {
                min: Some(0.0),
                max: None,
                excl_min: false,
                excl_max: false,
            }],
            ..bare_entry_field("balance", Tref::Prim(Prim::I64), vec![Source::With])
        },
    );
    let out = text(&module);
    assert!(out.contains("let mut violations: Vec<Violation> = Vec::new();"));
    // Always: the @arg value needs no presence guard around the length check.
    assert!(out.contains("if s.api_key.chars().count() < 1 {"));
    assert!(out.contains("field: \"api_key\".to_string()"));
    // String: guarded on the resolved value differing from its zero.
    assert!(out.contains("if s.region != String::new() && s.region.chars().count() < 1 {"));
    // Numeric: guarded on the chain having actually resolved.
    assert!(out.contains("if (weight_err.is_none() || s.weight != 0) && s.weight < 0 {"));
    // Bytes: guarded on the resolved value being non-empty; length is byte count.
    assert!(out.contains("if !s.receipt.is_empty() && s.receipt.len() < 1 {"));
    // Wide numeric: same shape as the narrow case, no literal suffix in Rust.
    assert!(out.contains("if (balance_err.is_none() || s.balance != 0) && s.balance < 0 {"));
    assert!(out.contains("if !violations.is_empty() {"));
    assert!(out.contains("return Err(TonoError::Validation(ValidationError { violations }));"));
}

#[test]
fn a_multi_entry_module_prefixes_settings_descriptors_and_discriminators() {
    let mut module = simple_entry_module();
    let op2 = Shape {
        id: "m#other.ping".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: Some(Box::new(WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![crate::ir::TemplatePart::Lit("/ping".into())]),
                body: None,
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: Some(WireValue::Field(vec!["token".into()])),
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
        },
        traits: vec![Trait {
            id: "http".into(),
            value: serde_json::json!({"method": "GET", "path": "/ping"}),
        }],
    };
    module.shapes.push(Shape {
        id: "m#other".into(),
        kind: ShapeKind::Entry {
            fields: vec![bare_entry_field(
                "token",
                Tref::Prim(Prim::String),
                vec![Source::Arg],
            )],
            operations: vec![op2],
        },
        traits: vec![],
    });
    let out = text(&module);
    assert!(out.contains("pub(crate) struct ClientSettings {"));
    assert!(out.contains("pub(crate) struct OtherSettings {"));
    assert!(out.contains("pub fn decode_client_create_charge_error("));
}

#[test]
fn retry_and_timeout_fields_read_typed_settings_with_the_seam_on_the_client() {
    let mut module = simple_entry_module();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "max_conns",
            Tref::Prim(Prim::I32),
            vec![Source::Default(serde_json::json!(5))],
        ),
    );
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations.iter_mut() {
                if let ShapeKind::Operation { wire: Some(w), .. } = &mut op.kind {
                    w.retry = Some(vec!["max_conns".into()]);
                }
            }
        }
    }
    let out = text(&module);
    // The retry maximum reads the typed field at the call site; the timing
    // seam rides the client as crate-visible fields (the parity harness, a
    // test module of the same crate, pins them), defaulted at construction.
    assert!(out.contains("let max_retries = resolve_max_retries(self.settings.max_conns as i64);"));
    assert!(out.contains("pub(crate) sleep: SleepFn,"));
    assert!(out.contains("pub(crate) random: RandomFn,"));
    assert!(out.contains("sleep: default_sleep(), random: default_random()"));
    // No runtime value bag is left anywhere in the construction.
    assert!(!out.contains("values.insert"));
}

#[test]
fn a_typed_bespoke_impl_calls_the_bound_symbol_and_guards_the_boundary() {
    let mut module = simple_entry_module();
    push_client_op(&mut module, "ping", None, None, vec![]);
    push_impl_extension(&mut module, "ping", false, "ext/rust/ping.rs#ping");
    let out = text(&module);
    assert!(out.contains("pub fn ping(&self) -> Result<(), TonoError> {"));
    assert!(
        out.contains("ping(&self.settings).map_err(|cause| match cause.downcast::<TonoError>() {")
    );
    assert!(out.contains("Ok(declared) => *declared,"));
    assert!(out.contains(
        "TonoError::Contract(ContractError { contract_name: \"ping\".to_string(), cause: other }),"
    ));
}

#[test]
fn a_typed_bespoke_impl_with_an_input_passes_it_alongside_settings() {
    let mut module = simple_entry_module();
    push_client_op(
        &mut module,
        "archive",
        Some(Tref::Ref {
            id: "m#charge".into(),
            args: vec![],
        }),
        None,
        vec![],
    );
    push_impl_extension(&mut module, "archive", false, "ext/rust/archive.rs#archive");
    let out = text(&module);
    assert!(out.contains("pub fn archive(&self, input: Charge) -> Result<(), TonoError> {"));
    assert!(out.contains("archive(&self.settings, input).map_err(|cause| "));
    assert!(out.contains("fn archive(settings: &Settings, input: Charge) -> Result<(), Box<dyn std::error::Error + Send + Sync>>"));
}

#[test]
fn a_raw_bespoke_impl_with_no_input_and_no_declared_errors_falls_back_to_undeclared() {
    let mut module = simple_entry_module();
    push_client_op(&mut module, "save", None, None, vec![]);
    push_impl_extension(&mut module, "save", true, "ext/rust/save.rs#save");
    let out = text(&module);
    assert!(out.contains("pub fn save(&self) -> Result<(), TonoError> {"));
    assert!(out.contains("let payload: Vec<u8> = Vec::new();"));
    assert!(out.contains(
        "let outcome = save(&self.settings, payload).map_err(|cause| match cause.downcast::<TonoError>() {"
    ));
    assert!(out.contains("Ok(declared) => *declared,"));
    assert!(out.contains(
        "TonoError::Contract(ContractError { contract_name: \"save\".to_string(), cause: other }),"
    ));
    assert!(out.contains("let raw_body = String::from_utf8_lossy(&outcome.body).into_owned();"));
    assert!(out.contains("if !outcome.success {"));
    assert!(out.contains(
        "return Err(TonoError::Api(APIFailure::Undeclared(APIError { status: 0, body: raw_body })));"
    ));
    assert!(out.contains("Ok(())"));
    assert!(out.contains(
        "fn save(settings: &Settings, payload: Vec<u8>) -> Result<tono_ext::Outcome, Box<dyn std::error::Error + Send + Sync>>"
    ));
}

#[test]
fn a_raw_bespoke_impl_with_an_input_marshals_it_to_the_payload() {
    let mut module = simple_entry_module();
    push_client_op(
        &mut module,
        "archive",
        Some(Tref::Ref {
            id: "m#charge".into(),
            args: vec![],
        }),
        None,
        vec![],
    );
    push_impl_extension(&mut module, "archive", true, "ext/rust/archive.rs#archive");
    let out = text(&module);
    assert!(out.contains("pub fn archive(&self, input: Charge) -> Result<(), TonoError> {"));
    assert!(out.contains(
        "let payload = serde_json::to_vec(&input).map_err(|e| TonoError::Decode(DecodeError { path: \"$\".to_string(), expected: \"input\".to_string(), raw: e.to_string() }))?;"
    ));
    assert!(out.contains("let outcome = archive(&self.settings, payload)"));
}

#[test]
fn a_raw_bespoke_impl_with_declared_errors_discriminates_by_code_alone() {
    let mut module = simple_entry_module();
    push_client_op(
        &mut module,
        "save",
        None,
        None,
        vec![Tref::Ref {
            id: "m#payment_declined".into(),
            args: vec![],
        }],
    );
    push_impl_extension(&mut module, "save", true, "ext/rust/save.rs#save");
    let out = text(&module);
    assert!(out.contains("pub fn decode_save_error(code: Option<&str>, body: &str) -> TonoError {"));
    assert!(out.contains("if code == Some(\"payment_declined\") {"));
    // The raw op's own body calls the discriminator by code, not status.
    assert!(out.contains("return Err(decode_save_error(Some(&outcome.code), &raw_body));"));
}

#[test]
fn a_raw_bespoke_impl_with_a_declared_output_decodes_the_success_body() {
    let mut module = simple_entry_module();
    push_client_op(
        &mut module,
        "save",
        None,
        Some(Tref::Ref {
            id: "m#charge".into(),
            args: vec![],
        }),
        vec![],
    );
    push_impl_extension(&mut module, "save", true, "ext/rust/save.rs#save");
    let out = text(&module);
    assert!(out.contains("pub fn save(&self) -> Result<Charge, TonoError> {"));
    // Charge has a required member, so the success body decodes through the
    // per-type shared decoder (the same one `create_charge`'s protocol path
    // calls) rather than an inline probe repeated at this call site.
    assert!(out.contains("decode_charge(&raw_body)"));
}

#[test]
fn a_client_init_hook_runs_after_source_resolution_and_its_error_is_guarded() {
    let mut module = simple_entry_module();
    module.extensions.push(Extension {
        name: "client_init".into(),
        kind: ExtKind::Hook,
        signature: None,
        raw: false,
        bindings: [(
            "rust".to_string(),
            "ext/rust/init.rs#init_client".to_string(),
        )]
        .into_iter()
        .collect(),
        conformance: None,
    });
    let out = text(&module);
    assert!(out.contains(
        "if let Err(e) = init_client(&mut s) {\n            return Err(match e.downcast::<TonoError>() {\n                Ok(declared) => *declared,\n                Err(other) => TonoError::Contract(ContractError { contract_name: \"client_init\".to_string(), cause: other }),\n            });\n        }"
    ));
}

#[test]
fn with_no_hooks_bound_the_ops_invoke_no_hook_at_all() {
    let module = simple_entry_module();
    let out = text(&module);
    assert!(!out.contains("before_request"));
    assert!(!out.contains("after_response"));
}

#[test]
fn a_before_request_hook_is_called_inline_leaving_after_response_unset() {
    let mut module = simple_entry_module();
    push_hook_extension(
        &mut module,
        "before_request",
        "ext/rust/sign.rs#sign_request",
    );
    let out = text(&module);
    // The bespoke symbol is awaited directly on the built request, and its
    // failure classifies at the call site under the slot's name.
    assert!(out.contains("let request = match sign_request(request).await {"));
    assert!(out.contains(
        "TonoError::Contract(ContractError { contract_name: \"before_request\".to_string(), cause: other }),"
    ));
    assert!(!out.contains("after_response"));
}

#[test]
fn an_after_response_hook_is_called_inline_leaving_before_request_unset() {
    let mut module = simple_entry_module();
    push_hook_extension(
        &mut module,
        "after_response",
        "ext/rust/log.rs#log_response",
    );
    let out = text(&module);
    assert!(out.contains("let response = match log_response(response).await {"));
    assert!(out.contains(
        "TonoError::Contract(ContractError { contract_name: \"after_response\".to_string(), cause: other }),"
    ));
    assert!(!out.contains("before_request"));
}

#[test]
fn both_lifecycle_request_hooks_bound_together_wire_both_slots() {
    let mut module = simple_entry_module();
    push_hook_extension(
        &mut module,
        "before_request",
        "ext/rust/sign.rs#sign_request",
    );
    push_hook_extension(
        &mut module,
        "after_response",
        "ext/rust/log.rs#log_response",
    );
    let out = text(&module);
    assert!(out.contains("let request = match sign_request(request).await {"));
    assert!(out.contains("let response = match log_response(response).await {"));
}

#[test]
fn a_bytes_field_sourced_from_env_pulls_in_the_base64_decoder() {
    let mut module = simple_entry_module();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "secret",
            Tref::Prim(Prim::Bytes),
            vec![
                Source::Env(EnvName::Name("SECRET".into())),
                Source::Default(serde_json::json!("")),
            ],
        ),
    );
    let out = text(&module);
    // `entry_text` renders declarations in isolation, without the
    // assembler's cross-group import resolution (see
    // `crate::codegen::pipeline_tests::rust_entry_resolution_helpers_prune_to_only_what_the_model_uses`
    // for the full-pipeline proof the `use` this call site pulls in lands);
    // this asserts only the call site itself.
    assert!(out.contains("match base64_bytes::decode(&v) {"));
}

#[test]
fn an_operation_with_neither_a_descriptor_nor_an_impl_fails_loudly() {
    let mut module = simple_entry_module();
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            if shape.id == "m#client" {
                operations.push(Shape {
                    id: "m#client.local".into(),
                    kind: ShapeKind::Operation {
                        input_name: None,
                        input: None,
                        output: None,
                        errors: vec![],
                        wire: None,
                    },
                    traits: vec![],
                });
            }
        }
    }
    let out = text(&module);
    assert!(out.contains("pub fn local(&self) -> Result<(), TonoError> {"));
    assert!(out.contains("operation has no implementation for Rust"));
}

#[test]
fn a_timeout_duration_field_converts_to_milliseconds_eagerly() {
    let mut module = simple_entry_module();
    push_entry_field(
        &mut module,
        bare_entry_field(
            "wait",
            Tref::Prim(Prim::Duration),
            vec![Source::Default(serde_json::json!("5s"))],
        ),
    );
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations.iter_mut() {
                if let ShapeKind::Operation { wire: Some(w), .. } = &mut op.kind {
                    w.timeout = Some(vec!["wait".into()]);
                }
            }
        }
    }
    let out = text(&module);
    // The constructor converts once (a malformed value is a ConfigError at
    // construction); the op reads the already-converted private field. See
    // the note on the base64 test above: the `use` this call site pulls in
    // is proven at the full-pipeline level, not here.
    assert!(out.contains("let wait_ms = match parse_duration_ms(&s.wait.0) {"));
    assert!(out.contains("wait: invalid duration"));
    assert!(out.contains("http_send_with_timeout(&self.options, request, self.wait_ms)"));
}

#[test]
fn the_matrix_module_exercises_every_resolution_idiom() {
    // "api" is an initialism the shared casing engine upcases, matching
    // every other target's `HTTPCode`-style handling.
    let module = crate::codegen::test_support::entries_matrix_module();
    let out = text(&module);
    assert!(out.contains("pub struct API {"));
    // The matrix module has @with fields, so it builds through a builder.
    assert!(out.contains("s.u = self.u;"));

    // Typed env boundaries: every narrow-int width spells its own parse and
    // range guard, plus float, bool, and duration.
    assert!(out.contains("match v.parse::<i8>() {"));
    assert!(out.contains("match v.parse::<i16>() {"));
    assert!(out.contains("match v.parse::<i64>() {"));
    assert!(out.contains("match v.parse::<u8>() {"));
    assert!(out.contains("match v.parse::<u64>() {"));
    assert!(out.contains("match v.parse::<f64>() {"));
    assert!(out.contains("\"true\" | \"1\" => { s.flag = true; }"));
    assert!(out.contains("if parse_duration_ms(&v).is_err() {"));
    // An enum field is a branded type: cast at the boundary (Unknown catches
    // an undeclared wire value).
    assert!(out.contains("s.mode = Mode::Unknown(v);"));
    // Guaranteed and error-tracked dynamic env names both spell one balanced run.
    assert!(out.contains("read_env(&s.naming)"));
    assert!(out.contains(
        "dynamic_err = naming_err.as_ref().map(|e| ConfigError { message: format!(\"naming <- {}\", e.message) });"
    ));
    // Transforms compose innermost-first; the shared casing helpers chain
    // around the target-specific trim/lower/upper.
    assert!(out.contains(
        "str_upper_snake(str_pascal(str_kebab(str_snake((((format!(\"k-{}{}\", s.naming, s.tiny.to_string())).trim().to_string()).to_lowercase()).to_uppercase()))))"
    ));
    // Both select flavors: why-tracked with an inline source arm and a
    // fallback chain, and a guaranteed one that fails construction on an
    // undeclared value via Display-stringified match.
    assert!(out.contains("match (s.naming).to_string().as_str() {"));
    assert!(out.contains("\"a\" => {"));
    assert!(out.contains("s.picked = \"one\".to_string();"));
    assert!(out.contains("match (s.sure_name).to_string().as_str() {"));
    assert!(out.contains(
        "return Err(TonoError::Config(ConfigError { message: format!(\"sure_pick: match on sure_name: unmatched value {}\", s.sure_name) }));"
    ));
    // Composition: a config struct built member by member (a bind layered
    // over a sibling chain, a plain env member, a narrow-int member parse).
    assert!(out.contains("let mut composed = Conf {"));
    assert!(out.contains("composed.key = (s.naming).clone();"));
    assert!(out.contains("composed.sure = (s.sure_name).clone();"));
    assert!(out.contains("match v.parse::<i32>() {"));
    // Structured and whole-JSON sources: an explicit @with value wins over
    // the env fallback, and the structured one probes its required member
    // before the strict decode.
    assert!(out.contains("if let Some(v) = self.creds.clone() {"));
    assert!(out.contains("if probe.get(\"token\").map(|v| v.is_null()).unwrap_or(true) {"));
    assert!(out.contains("let decoded: Credentials = match serde_json::from_str(&raw) {"));
    assert!(out.contains("if let Some(v) = self.labels.clone() {"));
    assert!(out.contains("Ok(v) => { s.labels = v; }"));
    // Consumed-chain requires: a string-like field, a duration field (compared
    // against its branded zero, which needs PartialEq), and a numeric field
    // that only fails when its chain reported absence.
    assert!(out.contains("if s.naming == String::new() {"));
    assert!(out.contains("if s.wait == Duration(String::new()) {"));
    assert!(out.contains("if let Some(tiny_err) = &tiny_err {"));
    // An unread error var (nothing downstream consumes it) is explicitly discarded.
    assert!(out.contains("let _ = &small_err;"));
    // The `@timeout` duration converts to milliseconds eagerly at
    // construction (guarded on presence: `wait` resolves only from env), and
    // the retry maximum reads its typed field at the call site with the URL
    // reading the typed endpoint.
    assert!(out.contains("let wait_ms = if s.wait != Duration(String::new()) {"));
    assert!(out.contains("match parse_duration_ms(&s.wait.0) {"));
    assert!(out.contains("let max_retries = resolve_max_retries(self.settings.tiny as i64);"));
    assert!(out.contains("http_send_with_timeout(&self.options, request, self.wait_ms)"));
    assert!(out.contains("set_header(&mut headers, \"X-K\", self.settings.derived.clone());"));
    assert!(out.contains(
        "format!(\"{}/x/{}\", self.settings.naming, percent_path(&self.settings.naming))"
    ));

    // The four method shapes: a full wire binding with input/output, a bare
    // one with neither, a primitive (non-struct) output, and a bespoke stub
    // with no binding for this target.
    assert!(
        out.contains("pub async fn fetch_note(&self, input: Note) -> Result<Note, TonoError> {")
    );
    assert!(out.contains("pub async fn ping(&self) -> Result<(), TonoError> {"));
    assert!(out.contains("pub async fn count(&self) -> Result<i32, TonoError> {"));
    assert!(out.contains(
        "serde_json::from_str::<i32>(body).map_err(|_| TonoError::Decode(DecodeError { path: \"$\".to_string(), expected: \"i32\".to_string(), raw: body.to_string() }))"
    ));
    assert!(out.contains("operation has no implementation for Rust"));
    // The entry doc rides the client type.
    assert!(out.contains("/// The matrix entry."));
}

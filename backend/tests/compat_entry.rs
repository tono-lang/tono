//! Compatibility of the construction surface: entries, configs, their value
//! sources, selection tables, bindings, and the operations declared in an entry
//! body. One test per category boundary, since the categories are the contract.

use serde_json::json;
use tono_backend::compat::{diff, Category, Report};
use tono_backend::ir::{
    ArmValue, Bind, Constraint, EntryField, EnvName, Model, Module, Prim, Select, SelectArm, Shape,
    ShapeKind, Source, Trait, Tref,
};

fn model(shapes: Vec<Shape>) -> Model {
    Model {
        tono_ir_version: 6,
        modules: vec![Module {
            tests: vec![],
            name: "billing".into(),
            extensions: vec![],
            ext_libs: vec![],
            shapes,
            operations: vec![],
        }],
    }
}

fn field(name: &str, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target: Tref::Prim(Prim::String),
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn entry(fields: Vec<EntryField>, operations: Vec<Shape>) -> Model {
    model(vec![Shape {
        id: "billing#client".into(),
        kind: ShapeKind::Entry { fields, operations },
        traits: vec![],
    }])
}

fn config(fields: Vec<EntryField>) -> Model {
    model(vec![Shape {
        id: "billing#conf".into(),
        kind: ShapeKind::Config { fields },
        traits: vec![],
    }])
}

fn op(id: &str, traits: Vec<Trait>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits,
    }
}

fn trait_of(id: &str, value: serde_json::Value) -> Trait {
    Trait {
        id: id.into(),
        value,
    }
}

fn env(name: &str) -> Source {
    Source::Env(EnvName::Name(name.into()))
}

fn keys(report: &Report) -> Vec<&str> {
    report.changes.iter().map(|c| c.key.as_str()).collect()
}

fn category(report: &Report, key: &str) -> Category {
    report
        .changes
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("no change with key {key:?} in {:?}", keys(report)))
        .category
}

// ── The construction surface: what a caller writes ──────────────────────────

#[test]
fn removing_an_explicit_field_is_source_breaking_but_an_internal_one_is_behavioral() {
    let before = entry(
        vec![
            field("api_key", vec![Source::Arg]),
            field("timeout", vec![Source::With]),
            field("endpoint_env", vec![env("ENDPOINT_ENV")]),
        ],
        vec![],
    );
    let after = entry(vec![], vec![]);
    let report = diff(&before, &after);
    assert_eq!(
        category(&report, "remove-field billing#client.api_key"),
        Category::SourceBreaking
    );
    assert_eq!(
        category(&report, "remove-field billing#client.timeout"),
        Category::SourceBreaking
    );
    assert_eq!(
        category(&report, "remove-field billing#client.endpoint_env"),
        Category::Behavioral
    );
}

#[test]
fn adding_an_arg_is_source_breaking_but_a_with_or_internal_field_is_safe() {
    let before = entry(vec![], vec![]);
    let after = entry(
        vec![
            field("api_key", vec![Source::Arg]),
            field("timeout", vec![Source::With]),
            field("endpoint", vec![env("ENDPOINT")]),
        ],
        vec![],
    );
    let report = diff(&before, &after);
    assert_eq!(
        category(&report, "add-field billing#client.api_key"),
        Category::SourceBreaking
    );
    assert_eq!(
        category(&report, "add-field billing#client.timeout"),
        Category::AdditiveSafe
    );
    assert_eq!(
        category(&report, "add-field billing#client.endpoint"),
        Category::AdditiveSafe
    );
}

#[test]
fn reordering_positional_args_is_source_breaking() {
    let before = entry(
        vec![
            field("api_key", vec![Source::Arg]),
            field("region", vec![Source::Arg]),
        ],
        vec![],
    );
    let after = entry(
        vec![
            field("region", vec![Source::Arg]),
            field("api_key", vec![Source::Arg]),
        ],
        vec![],
    );
    let report = diff(&before, &after);
    let change = report
        .changes
        .iter()
        .find(|c| c.key == "reorder-args billing#client")
        .expect("expected a reorder change");
    assert_eq!(change.category, Category::SourceBreaking);
    assert_eq!(change.detail, "[api_key, region] -> [region, api_key]");
    // Reordering @with fields is not positional, so it is not a change at all.
    let before = entry(
        vec![
            field("a", vec![Source::With]),
            field("b", vec![Source::With]),
        ],
        vec![],
    );
    let after = entry(
        vec![
            field("b", vec![Source::With]),
            field("a", vec![Source::With]),
        ],
        vec![],
    );
    assert!(diff(&before, &after).changes.is_empty());
}

#[test]
fn moving_a_field_between_arg_and_with_is_source_breaking() {
    let before = entry(vec![field("timeout", vec![Source::Arg])], vec![]);
    let after = entry(vec![field("timeout", vec![Source::With])], vec![]);
    let report = diff(&before, &after);
    let change = report
        .changes
        .iter()
        .find(|c| c.key == "change-surface billing#client.timeout")
        .expect("expected a surface change");
    assert_eq!(change.category, Category::SourceBreaking);
    assert_eq!(change.detail, "[arg] -> [with]");
}

#[test]
fn retyping_an_entry_field_is_source_breaking() {
    let before = entry(vec![field("max_retries", vec![Source::With])], vec![]);
    let mut retyped = field("max_retries", vec![Source::With]);
    retyped.target = Tref::Prim(Prim::I32);
    let after = entry(vec![retyped], vec![]);
    assert_eq!(
        category(
            &diff(&before, &after),
            "retype-field billing#client.max_retries"
        ),
        Category::SourceBreaking
    );
}

// ── Resolution: where a value comes from ────────────────────────────────────

#[test]
fn changing_an_env_name_or_a_default_is_behavioral() {
    let before = entry(
        vec![field(
            "endpoint",
            vec![env("ENDPOINT"), Source::Default(json!("https://a"))],
        )],
        vec![],
    );
    let after = entry(
        vec![field(
            "endpoint",
            vec![env("ENDPOINT_V2"), Source::Default(json!("https://b"))],
        )],
        vec![],
    );
    assert_eq!(
        category(
            &diff(&before, &after),
            "change-source billing#client.endpoint"
        ),
        Category::Behavioral
    );
}

#[test]
fn adding_a_default_behind_an_explicit_source_keeps_the_surface() {
    let before = entry(vec![field("timeout", vec![Source::With])], vec![]);
    let after = entry(
        vec![field(
            "timeout",
            vec![Source::With, Source::Default(json!("10s"))],
        )],
        vec![],
    );
    let report = diff(&before, &after);
    // The option is still there and still spelled the same way; only the value
    // it falls back to is new.
    assert!(
        !keys(&report)
            .iter()
            .any(|k| k.starts_with("change-surface")),
        "unexpected surface change: {:?}",
        keys(&report)
    );
    assert_eq!(
        category(&report, "change-source billing#client.timeout"),
        Category::Behavioral
    );
}

#[test]
fn changing_a_format_template_or_a_transform_pipeline_is_behavioral() {
    let mut base = field("client_key", vec![Source::Arg]);
    base.transforms = vec!["trim".into()];
    let mut curr = base.clone();
    curr.transforms = vec!["trim".into(), "upper_snake".into()];
    let report = diff(
        &entry(vec![base.clone()], vec![]),
        &entry(vec![curr], vec![]),
    );
    let change = report
        .changes
        .iter()
        .find(|c| c.key == "change-transforms billing#client.client_key")
        .expect("expected a transforms change");
    assert_eq!(change.category, Category::Behavioral);
    assert_eq!(change.detail, "[trim] -> [trim, upper_snake]");

    let mut curr = base.clone();
    curr.format = Some(vec![tono_backend::ir::TemplatePart::Lit("X".into())]);
    assert_eq!(
        category(
            &diff(&entry(vec![base], vec![]), &entry(vec![curr], vec![])),
            "change-format billing#client.client_key"
        ),
        Category::Behavioral
    );
}

#[test]
fn changing_a_selection_table_is_behavioral() {
    let table = |arm: &str| Select {
        subject: vec!["version".into()],
        arms: vec![SelectArm {
            pattern: Some(json!(arm)),
            value: ArmValue::Field(vec!["endpoint_v1".into()]),
        }],
    };
    let mut base = field("endpoint", vec![]);
    base.select = Some(table("v1"));
    let mut curr = field("endpoint", vec![]);
    curr.select = Some(table("v2"));
    assert_eq!(
        category(
            &diff(&entry(vec![base], vec![]), &entry(vec![curr], vec![])),
            "change-select billing#client.endpoint"
        ),
        Category::Behavioral
    );
}

#[test]
fn changing_a_composition_binding_is_behavioral() {
    let mut base = field("conf", vec![]);
    base.binds = vec![Bind {
        field: "api_key".into(),
        source: vec!["api_key".into()],
    }];
    let mut curr = field("conf", vec![]);
    curr.binds = vec![Bind {
        field: "api_key".into(),
        source: vec!["client_key".into()],
    }];
    assert_eq!(
        category(
            &diff(&config(vec![base]), &config(vec![curr])),
            "change-bind billing#conf.conf"
        ),
        Category::Behavioral
    );
}

#[test]
fn tightening_an_entry_field_constraint_is_behavioral_and_deprecating_it_is_additive() {
    let mut base = field("api_key", vec![Source::Arg]);
    base.constraints = vec![Constraint::Length {
        min: Some(1),
        max: None,
    }];
    let mut curr = base.clone();
    curr.constraints = vec![Constraint::Length {
        min: Some(8),
        max: None,
    }];
    curr.traits = vec![trait_of("deprecated", json!(["use client_key"]))];
    let report = diff(&entry(vec![base], vec![]), &entry(vec![curr], vec![]));
    assert_eq!(
        category(&report, "tighten-constraint billing#client.api_key"),
        Category::Behavioral
    );
    assert_eq!(
        category(&report, "add-deprecated billing#client.api_key"),
        Category::AdditiveSafe
    );
}

// ── Operations declared in an entry body ────────────────────────────────────

#[test]
fn adding_an_entry_op_is_safe() {
    let before = entry(vec![], vec![]);
    let after = entry(vec![], vec![op("billing#client.save", vec![])]);
    assert_eq!(
        category(&diff(&before, &after), "add-entry-op billing#client.save"),
        Category::AdditiveSafe
    );
}

#[test]
fn an_entry_op_signature_change_is_source_breaking() {
    let before = entry(vec![], vec![op("billing#client.save", vec![])]);
    let after = entry(
        vec![],
        vec![Shape {
            id: "billing#client.save".into(),
            kind: ShapeKind::Operation {
                input_name: None,
                input: Some(Tref::Ref {
                    id: "billing#Note".into(),
                    args: vec![],
                }),
                output: None,
                errors: vec![],
                wire: None,
                impl_call: None,
            },
            traits: vec![],
        }],
    );
    assert_eq!(
        category(
            &diff(&before, &after),
            "change-operation billing#client.save$input"
        ),
        Category::SourceBreaking
    );
}

#[test]
fn changing_the_http_method_or_path_is_wire_breaking() {
    let http =
        |method: &str, path: &str| trait_of("http", json!({ "method": method, "path": path }));
    let before = entry(
        vec![],
        vec![op("billing#client.save", vec![http("POST", "/notes")])],
    );
    let after = entry(
        vec![],
        vec![op("billing#client.save", vec![http("PUT", "/notes/v2")])],
    );
    let report = diff(&before, &after);
    let method = report
        .changes
        .iter()
        .find(|c| c.key == "change-http billing#client.save$method")
        .expect("expected a method change");
    assert_eq!(method.category, Category::WireBreaking);
    assert_eq!(method.detail, "method POST -> PUT");
    assert_eq!(
        category(&report, "change-http billing#client.save$path"),
        Category::WireBreaking
    );
}

#[test]
fn repointing_the_endpoint_or_the_retry_budget_is_behavioral() {
    let before = entry(
        vec![],
        vec![op(
            "billing#client.save",
            vec![
                trait_of(
                    "http",
                    json!({ "method": "POST", "path": "/notes", "endpoint": { "field": ["endpoint"] } }),
                ),
                trait_of("retry", json!([{ "field": ["max_retries"] }])),
            ],
        )],
    );
    let after = entry(
        vec![],
        vec![op(
            "billing#client.save",
            vec![
                trait_of(
                    "http",
                    json!({ "method": "POST", "path": "/notes", "endpoint": { "field": ["endpoint_v2"] } }),
                ),
                trait_of("retry", json!([{ "field": ["retries"] }])),
            ],
        )],
    );
    let report = diff(&before, &after);
    assert_eq!(
        category(&report, "change-http billing#client.save$endpoint"),
        Category::Behavioral
    );
    assert_eq!(
        category(&report, "change-descriptor billing#client.save$retry"),
        Category::Behavioral
    );
    assert!(
        !keys(&report).iter().any(|k| k.contains("$method")),
        "the request itself did not change: {:?}",
        keys(&report)
    );
}

#[test]
fn an_operation_that_moves_to_a_bespoke_impl_is_judged_by_its_contract() {
    // The operation keeps its signature and its errors; only its implementation
    // moves from a protocol to a bound symbol. That is invisible to callers, so
    // the checker says nothing about it.
    let before = entry(
        vec![],
        vec![op(
            "billing#client.save",
            vec![trait_of(
                "http",
                json!({ "method": "POST", "path": "/notes" }),
            )],
        )],
    );
    let mut after = entry(vec![], vec![op("billing#client.save", vec![])]);
    after.modules[0].extensions = vec![tono_backend::ir::Extension {
        name: "client.save".into(),
        kind: tono_backend::ir::ExtKind::Impl,
        signature: None,
        raw: true,
        bindings: [("go".to_string(), "ext/go/n.go#Save".to_string())]
            .into_iter()
            .collect(),
        conformance: None,
    }];
    let report = diff(&before, &after);
    assert!(
        report.changes.is_empty(),
        "the contract is unchanged: {:?}",
        keys(&report)
    );
}

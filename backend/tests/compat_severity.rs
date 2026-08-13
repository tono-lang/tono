//! Compat severity/config and service-op tests, split out of `compat.rs` to
//! keep that file within the repo size gate.

use std::collections::BTreeMap;

use tono_backend::compat::{diff, Category, Change, Config, Report, Severity};
use tono_backend::ir::{Member, Model, Module, Prim, Shape, ShapeKind, Tref};

fn model(shapes: Vec<Shape>) -> Model {
    Model {
        tono_ir_version: 3,
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

fn member(name: &str, target: Tref, required: bool) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

fn charge(members: Vec<Member>) -> Model {
    model(vec![structure("billing#Charge", members)])
}

fn keys(report: &Report) -> Vec<&str> {
    report.changes.iter().map(|c| c.key.as_str()).collect()
}

fn find<'a>(report: &'a Report, key: &str) -> &'a Change {
    report
        .changes
        .iter()
        .find(|c| c.key == key)
        .unwrap_or_else(|| panic!("no change with key {key:?} in {:?}", keys(report)))
}

fn service_with_op(op_ids: Vec<&str>, keep_op_shape: bool) -> Model {
    let service = Shape {
        id: "billing#Api".into(),
        kind: ShapeKind::Service {
            operations: op_ids.iter().map(|s| s.to_string()).collect(),
        },
        traits: vec![],
    };
    let operations = if keep_op_shape {
        vec![Shape {
            id: "billing#Op".into(),
            kind: ShapeKind::Operation {
                input_name: None,
                input: None,
                output: None,
                errors: vec![],
                wire: None,
            },
            traits: vec![],
        }]
    } else {
        vec![]
    };
    Model {
        tono_ir_version: 3,
        modules: vec![Module {
            tests: vec![],
            name: "billing".into(),
            extensions: vec![],
            ext_libs: vec![],
            shapes: vec![service],
            operations,
        }],
    }
}

#[test]
fn a_deleted_operation_is_reported_once_not_twice() {
    let before = service_with_op(vec!["billing#Op"], true);

    // Deleted entirely: only the removed shape is reported, not a redundant
    // remove-service-op for the same fact.
    let deleted = service_with_op(vec![], false);
    let report = diff(&before, &deleted);
    assert!(keys(&report).contains(&"remove-shape billing#Op"));
    assert!(!keys(&report)
        .iter()
        .any(|k| k.starts_with("remove-service-op")));

    // Dropped from the service list but still defined elsewhere: that is the
    // genuine service-level break, so it is reported.
    let unexposed = service_with_op(vec![], true);
    assert_eq!(
        find(
            &diff(&before, &unexposed),
            "remove-service-op billing#Api/billing#Op"
        )
        .category,
        Category::SourceBreaking
    );
}

#[test]
fn worst_severity_applies_config_and_allowlist() {
    let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    let after = charge(vec![member("amount", Tref::Prim(Prim::String), true)]);
    let report = diff(&before, &after);

    // Default: a wire break is an error.
    assert_eq!(report.worst(&Config::default()), Severity::Error);

    // Allowlisting that exact change key drops it to Off.
    let allowed = Config {
        allow: vec!["retype billing#Charge.amount".into()],
        ..Config::default()
    };
    assert_eq!(report.worst(&allowed), Severity::Off);

    // Configuring the whole level down to a warning also applies.
    let mut severity = BTreeMap::new();
    severity.insert(Category::WireBreaking, Severity::Warn);
    let downgraded = Config {
        severity,
        allow: vec![],
    };
    assert_eq!(report.worst(&downgraded), Severity::Warn);
}

#[test]
fn category_and_severity_parse_their_cli_labels() {
    for c in [
        Category::WireBreaking,
        Category::SourceBreaking,
        Category::Behavioral,
        Category::AdditiveSafe,
    ] {
        assert_eq!(Category::parse(c.label()), Some(c));
    }
    assert_eq!(Category::parse("nonsense"), None);
    assert_eq!(Severity::parse("error"), Some(Severity::Error));
    assert_eq!(Severity::parse("warning"), Some(Severity::Warn));
    assert_eq!(Severity::parse("off"), Some(Severity::Off));
    assert_eq!(Severity::parse("loud"), None);
}

#[test]
fn config_deserializes_from_json() {
    let cfg: Config = serde_json::from_str(
        r#"{"severity":{"wireBreaking":"warn","behavioral":"off"},"allow":["retype a#B.c"]}"#,
    )
    .unwrap();
    assert_eq!(
        cfg.severity.get(&Category::WireBreaking),
        Some(&Severity::Warn)
    );
    assert_eq!(
        cfg.severity.get(&Category::Behavioral),
        Some(&Severity::Off)
    );
    assert_eq!(cfg.allow, vec!["retype a#B.c".to_string()]);
}

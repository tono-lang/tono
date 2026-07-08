//! Compatibility-checker tests. The `compat` API is fully public, so these live
//! as an integration test to keep the module file within the repo size gate.

use std::collections::BTreeMap;

use serde_json::json;
use tono_backend::compat::{diff, Category, Change, Config, Report, Severity};
use tono_backend::ir::{
    Constraint, EnumBacking, Member, Model, Module, Prim, Shape, ShapeKind, Trait, Tref,
};

fn model(shapes: Vec<Shape>) -> Model {
    Model {
        tono_ir_version: 2,
        modules: vec![Module {
            name: "billing".into(),
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

#[test]
fn identical_models_have_no_changes() {
    let m = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    assert!(diff(&m, &m).changes.is_empty());
}

#[test]
fn retyping_a_member_is_wire_breaking() {
    let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    let after = charge(vec![member("amount", Tref::Prim(Prim::String), true)]);
    let report = diff(&before, &after);
    let change = find(&report, "retype billing#Charge.amount");
    assert_eq!(change.category, Category::WireBreaking);
    assert_eq!(change.detail, "u64 -> string");
}

#[test]
fn removing_a_member_is_wire_breaking() {
    let before = charge(vec![
        member("amount", Tref::Prim(Prim::U64), true),
        member("note", Tref::Prim(Prim::String), false),
    ]);
    let after = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    assert_eq!(
        find(&diff(&before, &after), "remove-member billing#Charge.note").category,
        Category::WireBreaking
    );
}

#[test]
fn adding_an_optional_member_is_safe_but_a_required_one_breaks() {
    let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    let with_optional = charge(vec![
        member("amount", Tref::Prim(Prim::U64), true),
        member("note", Tref::Prim(Prim::String), false),
    ]);
    assert_eq!(
        find(
            &diff(&before, &with_optional),
            "add-member billing#Charge.note"
        )
        .category,
        Category::AdditiveSafe
    );

    let with_required = charge(vec![
        member("amount", Tref::Prim(Prim::U64), true),
        member("note", Tref::Prim(Prim::String), true),
    ]);
    assert_eq!(
        find(
            &diff(&before, &with_required),
            "add-member billing#Charge.note"
        )
        .category,
        Category::WireBreaking
    );
}

#[test]
fn making_a_member_required_breaks_but_optional_is_safe() {
    let optional = charge(vec![member("note", Tref::Prim(Prim::String), false)]);
    let required = charge(vec![member("note", Tref::Prim(Prim::String), true)]);
    assert_eq!(
        find(
            &diff(&optional, &required),
            "require-member billing#Charge.note"
        )
        .category,
        Category::WireBreaking
    );
    assert_eq!(
        find(
            &diff(&required, &optional),
            "optional-member billing#Charge.note"
        )
        .category,
        Category::AdditiveSafe
    );
}

#[test]
fn changing_a_wire_key_is_wire_breaking() {
    let mut renamed = member("amount", Tref::Prim(Prim::U64), true);
    renamed.traits = vec![Trait {
        id: "core#wire".into(),
        value: json!("amount_cents"),
    }];
    let before = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    let after = charge(vec![renamed]);
    assert_eq!(
        find(
            &diff(&before, &after),
            "change-wire billing#Charge.amount@wire"
        )
        .category,
        Category::WireBreaking
    );
}

#[test]
fn changing_a_default_is_behavioral() {
    let mut before_m = member("amount", Tref::Prim(Prim::U64), true);
    before_m.default = Some(Some(json!(0)));
    let mut after_m = member("amount", Tref::Prim(Prim::U64), true);
    after_m.default = Some(Some(json!(1)));
    assert_eq!(
        find(
            &diff(&charge(vec![before_m]), &charge(vec![after_m])),
            "change-default billing#Charge.amount"
        )
        .category,
        Category::Behavioral
    );
}

#[test]
fn tightening_a_range_is_behavioral_but_loosening_is_silent() {
    let with_max = |max: f64| {
        let mut m = member("amount", Tref::Prim(Prim::U64), true);
        m.constraints = vec![Constraint::Range {
            min: None,
            max: Some(max),
            excl_min: false,
            excl_max: false,
        }];
        charge(vec![m])
    };
    assert_eq!(
        find(
            &diff(&with_max(100.0), &with_max(50.0)),
            "tighten-constraint billing#Charge.amount"
        )
        .category,
        Category::Behavioral
    );
    // Loosening (raising the max) emits nothing.
    assert!(diff(&with_max(50.0), &with_max(100.0)).changes.is_empty());
}

#[test]
fn adding_a_constraint_is_behavioral() {
    let plain = charge(vec![member("name", Tref::Prim(Prim::String), true)]);
    let mut constrained_m = member("name", Tref::Prim(Prim::String), true);
    constrained_m.constraints = vec![Constraint::Length {
        min: None,
        max: Some(3),
    }];
    let constrained = charge(vec![constrained_m]);
    assert_eq!(
        find(
            &diff(&plain, &constrained),
            "add-constraint billing#Charge.name"
        )
        .category,
        Category::Behavioral
    );
}

fn enum_shape(id: &str, backing: EnumBacking, values: Vec<(String, Option<i64>)>) -> Model {
    model(vec![Shape {
        id: id.into(),
        kind: ShapeKind::Enum { backing, values },
        traits: vec![],
    }])
}

#[test]
fn narrowing_an_enum_breaks_but_widening_is_safe() {
    let two = enum_shape(
        "billing#Status",
        EnumBacking::String,
        vec![("open".into(), None), ("closed".into(), None)],
    );
    let one = enum_shape(
        "billing#Status",
        EnumBacking::String,
        vec![("open".into(), None)],
    );
    assert_eq!(
        find(
            &diff(&two, &one),
            "remove-enum-value billing#Status::closed"
        )
        .category,
        Category::WireBreaking
    );
    assert_eq!(
        find(&diff(&one, &two), "add-enum-value billing#Status::closed").category,
        Category::AdditiveSafe
    );
}

#[test]
fn changing_enum_backing_or_int_value_is_wire_breaking() {
    let s = enum_shape(
        "billing#Priority",
        EnumBacking::String,
        vec![("low".into(), None)],
    );
    let i = enum_shape(
        "billing#Priority",
        EnumBacking::Int,
        vec![("low".into(), Some(0))],
    );
    assert_eq!(
        find(&diff(&s, &i), "change-enum-backing billing#Priority").category,
        Category::WireBreaking
    );

    let i10 = enum_shape(
        "billing#Priority",
        EnumBacking::Int,
        vec![("low".into(), Some(10))],
    );
    assert_eq!(
        find(&diff(&i, &i10), "change-enum-value billing#Priority::low").category,
        Category::WireBreaking
    );
}

#[test]
fn changing_a_union_discriminator_is_wire_breaking() {
    let union = |disc: &str| {
        model(vec![Shape {
            id: "billing#Source".into(),
            kind: ShapeKind::Union {
                params: vec![],
                members: vec![member(
                    "card",
                    Tref::Ref {
                        id: "billing#Card".into(),
                        args: vec![],
                    },
                    true,
                )],
                discriminator: disc.into(),
            },
            traits: vec![],
        }])
    };
    assert_eq!(
        find(
            &diff(&union("type"), &union("kind")),
            "change-discriminator billing#Source@discriminator"
        )
        .category,
        Category::WireBreaking
    );
}

#[test]
fn removing_a_referenced_shape_is_wire_breaking_but_an_orphan_is_source_breaking() {
    let referenced = model(vec![
        structure(
            "billing#Charge",
            vec![member(
                "card",
                Tref::Ref {
                    id: "billing#Card".into(),
                    args: vec![],
                },
                true,
            )],
        ),
        structure(
            "billing#Card",
            vec![member("n", Tref::Prim(Prim::String), true)],
        ),
    ]);
    // Drop Card but keep the reference in Charge.
    let dropped = model(vec![structure(
        "billing#Charge",
        vec![member(
            "card",
            Tref::Ref {
                id: "billing#Card".into(),
                args: vec![],
            },
            true,
        )],
    )]);
    assert_eq!(
        find(&diff(&referenced, &dropped), "remove-shape billing#Card").category,
        Category::WireBreaking
    );

    // An orphan shape nobody references is only source-breaking.
    let orphan = model(vec![structure(
        "billing#Unused",
        vec![member("n", Tref::Prim(Prim::String), true)],
    )]);
    let empty = model(vec![]);
    assert_eq!(
        find(&diff(&orphan, &empty), "remove-shape billing#Unused").category,
        Category::SourceBreaking
    );
}

#[test]
fn an_operation_signature_change_is_source_breaking() {
    let op = |input: &str| Model {
        tono_ir_version: 2,
        modules: vec![Module {
            name: "billing".into(),
            shapes: vec![],
            operations: vec![Shape {
                id: "billing#Create".into(),
                kind: ShapeKind::Operation {
                    input: Some(Tref::Ref {
                        id: input.into(),
                        args: vec![],
                    }),
                    output: None,
                    errors: vec![],
                },
                traits: vec![],
            }],
        }],
    };
    assert_eq!(
        find(
            &diff(&op("billing#A"), &op("billing#B")),
            "change-operation billing#Create$input"
        )
        .category,
        Category::SourceBreaking
    );
}

#[test]
fn marking_a_shape_deprecated_is_additive() {
    let plain = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);
    let mut deprecated = structure(
        "billing#Charge",
        vec![member("amount", Tref::Prim(Prim::U64), true)],
    );
    deprecated.traits = vec![Trait {
        id: "core#deprecated".into(),
        value: json!("use v2"),
    }];
    assert_eq!(
        find(
            &diff(&plain, &model(vec![deprecated])),
            "add-deprecated billing#Charge"
        )
        .category,
        Category::AdditiveSafe
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

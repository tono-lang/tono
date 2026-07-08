//! Compatibility-checker tests. The `compat` API is fully public, so these live
//! as an integration test to keep the module file within the repo size gate.

use std::collections::BTreeMap;

use serde_json::json;
use tono_backend::compat::{diff, Category, Change, Config, Report, Severity};
use tono_backend::ir::{
    Constraint, EnumBacking, EnumValue, Member, Model, Module, Prim, Shape, ShapeKind, Trait, Tref,
};

fn model(shapes: Vec<Shape>) -> Model {
    Model {
        tono_ir_version: 3,
        modules: vec![Module {
            name: "billing".into(),
            extensions: vec![],
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
    // Cover both the frontend form (bare `wire` id, argument as a one-element
    // array) and the fixtures' form (`core#wire`, bare string): both must be
    // detected, otherwise a `@wire` change slips through on real IR.
    let with_wire = |id: &str, value| {
        let mut m = member("amount", Tref::Prim(Prim::U64), true);
        m.traits = vec![Trait {
            id: id.into(),
            value,
        }];
        charge(vec![m])
    };
    let plain = charge(vec![member("amount", Tref::Prim(Prim::U64), true)]);

    for after in [
        with_wire("wire", json!(["amount_cents"])),
        with_wire("core#wire", json!("amount_cents")),
    ] {
        let report = diff(&plain, &after);
        let change = find(&report, "change-wire billing#Charge.amount@wire");
        assert_eq!(change.category, Category::WireBreaking);
        assert_eq!(change.detail, "amount -> amount_cents");
    }
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
    let values = values
        .into_iter()
        .map(|(name, value)| EnumValue {
            name,
            value,
            traits: vec![],
        })
        .collect();
    model(vec![Shape {
        id: id.into(),
        kind: ShapeKind::Enum { backing, values },
        traits: vec![],
    }])
}

#[test]
fn narrowing_an_enum_is_source_breaking_and_widening_is_safe() {
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
    // Every enum is open, so a dropped value still decodes into the unknown arm on
    // the wire; only the consumer's named arm is gone, which is source-breaking.
    assert_eq!(
        find(
            &diff(&two, &one),
            "remove-enum-value billing#Status::closed"
        )
        .category,
        Category::SourceBreaking
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

fn source_union(variant_names: &[&str]) -> Model {
    let members = variant_names
        .iter()
        .map(|n| {
            member(
                n,
                Tref::Ref {
                    id: format!("billing#{n}"),
                    args: vec![],
                },
                true,
            )
        })
        .collect();
    model(vec![Shape {
        id: "billing#Source".into(),
        kind: ShapeKind::Union {
            params: vec![],
            members,
            discriminator: "type".into(),
        },
        traits: vec![],
    }])
}

#[test]
fn union_variants_use_variant_keys_not_member_keys() {
    let one = source_union(&["card"]);
    let two = source_union(&["card", "bank"]);

    // Adding a variant is additive (existing wire data still decodes); removing one
    // is a wire break. Both are reported with `*-variant` keys, not the misleading
    // member wording.
    let added = diff(&one, &two);
    let add = find(&added, "add-variant billing#Source::bank");
    assert_eq!(add.category, Category::AdditiveSafe);
    assert!(!keys(&added).iter().any(|k| k.starts_with("add-member")));

    let removed = diff(&two, &one);
    assert_eq!(
        find(&removed, "remove-variant billing#Source::bank").category,
        Category::WireBreaking
    );
    assert!(!keys(&removed)
        .iter()
        .any(|k| k.starts_with("remove-member")));
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
        tono_ir_version: 3,
        modules: vec![Module {
            name: "billing".into(),
            extensions: vec![],
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
fn marking_a_member_deprecated_is_additive() {
    // A field gaining @deprecated (the frontend's bare `deprecated` id) is safe,
    // reported at the member path, just like the shape-level case.
    let plain = charge(vec![member("fee", Tref::Prim(Prim::U64), true)]);
    let mut deprecated_member = member("fee", Tref::Prim(Prim::U64), true);
    deprecated_member.traits = vec![Trait {
        id: "deprecated".into(),
        value: json!(["folded into amount"]),
    }];
    assert_eq!(
        find(
            &diff(&plain, &charge(vec![deprecated_member])),
            "add-deprecated billing#Charge.fee"
        )
        .category,
        Category::AdditiveSafe
    );
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
                input: None,
                output: None,
                errors: vec![],
            },
            traits: vec![],
        }]
    } else {
        vec![]
    };
    Model {
        tono_ir_version: 3,
        modules: vec![Module {
            name: "billing".into(),
            extensions: vec![],
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

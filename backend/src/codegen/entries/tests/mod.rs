use super::*;
use crate::ir::{EnvName, FieldRef, Select, SelectArm};
use serde_json::json;

// The extern-call field-source tests live in their own submodule
// to keep this file under the line-count gate; they reuse the fixtures below
// through `super::*`.
mod call;
// The per-target capability gates over a `call:` line's shapes, likewise.
mod call_capabilities;
// The handle-method field-source tests, likewise.
mod handle_call;
// The `validate_entries` gate tests (generated-identifier collisions, the
// mixed loose/entry module rules) live in their own submodule for the same
// reason.
mod validate_gates;

fn field(name: &str, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target: Tref::Prim(crate::ir::Prim::String),
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

fn entry_shape(id: &str, fields: Vec<EntryField>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Entry {
            fields,
            operations: vec![],
        },
        traits: vec![],
    }
}

fn module_of(shapes: Vec<Shape>) -> Module {
    Module {
        tests: vec![],
        name: "m".into(),
        shapes,
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
    }
}

#[test]
fn fields_order_by_resolution_dependencies_keeping_declaration_order() {
    // endpoint selects on version and reads v1; v1 reads the variable named
    // by naming_field; naming_field formats from base.
    let mut endpoint = field("endpoint", vec![]);
    endpoint.select = Some(Select {
        subject_index: None,
        subject: vec!["version".into()],
        arms: vec![
            SelectArm {
                pattern: Some(json!("v1")),
                value: ArmValue::Field(vec!["v1".into()]),
            },
            SelectArm {
                pattern: None,
                value: ArmValue::Lit(json!("fallback")),
            },
        ],
    });
    let mut v1 = field("v1", vec![]);
    v1.sources = vec![Source::Env(EnvName::Field(FieldRef {
        field: vec!["naming_field".into()],
    }))];
    let mut naming_field = field("naming_field", vec![]);
    naming_field.format = Some(vec![
        TemplatePart::Lit("EP_".into()),
        TemplatePart::Field(vec!["base".into()]),
    ]);
    let base = field("base", vec![Source::Arg]);
    let version = field(
        "version",
        vec![Source::Env(EnvName::Name("V".into())), Source::With],
    );

    let module = module_of(vec![entry_shape(
        "m#client",
        vec![endpoint, v1, naming_field, base, version],
    )]);
    let entries = module_entries(&module);
    let order: Vec<&str> = entries[0].fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        order,
        vec!["base", "version", "naming_field", "v1", "endpoint"]
    );
}

#[test]
fn a_config_orders_after_the_fields_its_members_read() {
    // The config field is declared first, but one of its members formats from
    // the sibling `region`, so resolution must place `region` before the config.
    let mut label = field("label", vec![]);
    label.format = Some(vec![
        TemplatePart::Lit("r-".into()),
        TemplatePart::Field(vec!["region".into()]),
    ]);
    let config_shape = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config {
            fields: vec![label],
        },
        traits: vec![],
    };
    let mut conf = field("conf", vec![]);
    conf.target = Tref::Ref {
        id: "m#conf".into(),
        args: vec![],
    };
    let region = field("region", vec![Source::Env(EnvName::Name("REGION".into()))]);
    let module = module_of(vec![
        entry_shape("m#client", vec![conf, region]),
        config_shape,
    ]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["region", "conf"]);
}

#[test]
fn a_cycle_appends_the_rest_in_declaration_order_instead_of_dropping() {
    let mut a = field("a", vec![]);
    a.format = Some(vec![TemplatePart::Field(vec!["b".into()])]);
    let mut b = field("b", vec![]);
    b.format = Some(vec![TemplatePart::Field(vec!["a".into()])]);
    let module = module_of(vec![entry_shape("m#client", vec![a, b])]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["a", "b"]);
}

#[test]
fn args_and_with_fields_classify_by_source_in_declaration_order() {
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![
            field("key", vec![Source::Arg]),
            field("name", vec![Source::With, Source::Default(json!("demo"))]),
            field("plain", vec![Source::Env(EnvName::Name("P".into()))]),
            field("second", vec![Source::Arg]),
        ],
    )]);
    let entries = module_entries(&module);
    let args: Vec<&str> = entries[0].args().iter().map(|f| f.name.as_str()).collect();
    let configurable: Vec<&str> = entries[0]
        .with_fields()
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(args, vec!["key", "second"]);
    assert_eq!(configurable, vec!["name"]);
}

#[test]
fn value_paths_cover_fields_and_their_composed_members() {
    let conf = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config {
            fields: vec![field("api_key", vec![]), field("region", vec![])],
        },
        traits: vec![],
    };
    let mut settings = field("settings", vec![]);
    settings.target = Tref::Ref {
        id: "m#conf".into(),
        args: vec![],
    };
    let module = module_of(vec![
        conf,
        entry_shape(
            "m#client",
            vec![field("token", vec![Source::Arg]), settings],
        ),
    ]);
    let entries = module_entries(&module);
    let values = entries[0].value_paths(&module);
    let paths: Vec<&str> = values.iter().map(|v| v.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["token", "settings", "settings.api_key", "settings.region"]
    );
}

#[test]
fn companion_names_prefix_only_in_the_multi_entry_case() {
    assert_eq!(companion_name("client", "settings", false), "settings");
    assert_eq!(
        companion_name("client", "settings", true),
        "client_settings"
    );
}

#[test]
fn field_shapes_dispatch_on_the_referenced_shape_kind() {
    let conf = Shape {
        id: "m#conf".into(),
        kind: ShapeKind::Config { fields: vec![] },
        traits: vec![],
    };
    let creds = Shape {
        id: "m#creds".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![],
        },
        traits: vec![],
    };
    let mut composed = field("composed", vec![]);
    composed.target = Tref::Ref {
        id: "m#conf".into(),
        args: vec![],
    };
    let mut structured = field("structured", vec![]);
    structured.target = Tref::Ref {
        id: "m#creds".into(),
        args: vec![],
    };
    let mut labels = field("labels", vec![]);
    labels.target = Tref::Map(
        Box::new(Tref::Prim(crate::ir::Prim::String)),
        Box::new(Tref::Prim(crate::ir::Prim::String)),
    );
    let scalar = field("scalar", vec![]);
    let module = module_of(vec![
        conf,
        creds,
        entry_shape("m#client", vec![composed, structured, labels, scalar]),
    ]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let by_name = |n: &str| entry.fields.iter().find(|f| f.name == n).copied().unwrap();
    assert!(matches!(
        entry.field_shape(by_name("composed"), &module),
        FieldShape::Config(_)
    ));
    assert!(matches!(
        entry.field_shape(by_name("structured"), &module),
        FieldShape::Structured(_)
    ));
    assert!(matches!(
        entry.field_shape(by_name("labels"), &module),
        FieldShape::Json
    ));
    assert!(matches!(
        entry.field_shape(by_name("scalar"), &module),
        FieldShape::Scalar
    ));
}

#[test]
fn guaranteed_follows_arg_default_and_derivation_inputs() {
    let mut derived = field("derived", vec![]);
    derived.format = Some(vec![TemplatePart::Field(vec!["base".into()])]);
    let mut floating = field("floating", vec![]);
    floating.format = Some(vec![TemplatePart::Field(vec!["maybe".into()])]);
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![
            field("base", vec![Source::Arg]),
            field(
                "with_default",
                vec![Source::With, Source::Default(json!("d"))],
            ),
            field("maybe", vec![Source::Env(EnvName::Name("E".into()))]),
            derived,
            floating,
        ],
    )]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let by = |n: &str| entry.fields.iter().find(|f| f.name == n).copied().unwrap();
    assert!(entry.is_guaranteed(by("base")));
    assert!(entry.is_guaranteed(by("with_default")));
    assert!(!entry.is_guaranteed(by("maybe")));
    assert!(entry.is_guaranteed(by("derived")));
    assert!(!entry.is_guaranteed(by("floating")));
}

/// A bare entry whose one operation carries the given raw protocol traits.
fn entry_with_op_traits(traits: Vec<(&str, serde_json::Value)>) -> Module {
    let op = Shape {
        id: "m#client.save".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            output_nullable: false,
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits: traits
            .into_iter()
            .map(|(id, value)| crate::ir::Trait {
                id: id.into(),
                value,
            })
            .collect(),
    };
    module_of(vec![Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![],
            operations: vec![op],
        },
        traits: vec![],
    }])
}

#[test]
fn consumed_heads_read_the_raw_protocol_traits() {
    let module = entry_with_op_traits(vec![
        (
            "http",
            json!({
                "method": "POST",
                "path": "/v/{.tenant}/notes/{id}",
                "endpoint": {"field": ["endpoint"]}
            }),
        ),
        ("header", json!(["X-Client", {"field": ["client_name"]}])),
        ("timeout", json!([{"field": ["timeout"]}])),
        ("retry", json!([{"field": ["max_retries"]}])),
    ]);
    let entries = module_entries(&module);
    assert_eq!(
        entries[0].consumed_field_heads(),
        vec![
            "tenant",
            "endpoint",
            "client_name",
            "timeout",
            "max_retries"
        ]
    );
}

#[test]
fn consumed_paths_keep_the_member_segments() {
    let module = entry_with_op_traits(vec![
        (
            "http",
            json!({
                "method": "POST",
                "path": "/v/{.conf.tenant}/notes",
                "endpoint": {"field": ["endpoint"]}
            }),
        ),
        ("header", json!(["X-Key", {"field": ["conf", "api_key"]}])),
    ]);
    let entries = module_entries(&module);
    assert_eq!(
        entries[0].consumed_field_paths(),
        vec![
            vec!["conf".to_string(), "tenant".to_string()],
            vec!["endpoint".to_string()],
            vec!["conf".to_string(), "api_key".to_string()],
        ]
    );
    assert_eq!(entries[0].consumed_field_heads(), vec!["conf", "endpoint"]);
}

#[test]
fn an_env_ref_inside_a_match_arm_is_a_resolution_edge() {
    // The arm's inline @env(.naming) reads a sibling: the sibling must
    // resolve before the selecting field.
    let mut selector = field("selector", vec![]);
    selector.select = Some(Select {
        subject_index: None,
        subject: vec!["version".into()],
        arms: vec![
            SelectArm {
                pattern: Some(json!("v1")),
                value: ArmValue::Lit(json!("x")),
            },
            SelectArm {
                pattern: None,
                value: ArmValue::Sources(vec![Source::Env(EnvName::Field(FieldRef {
                    field: vec!["naming".into()],
                }))]),
            },
        ],
    });
    let naming = field("naming", vec![Source::Env(EnvName::Name("N".into()))]);
    let version = field("version", vec![Source::Default(json!("v1"))]);
    let module = module_of(vec![entry_shape(
        "m#client",
        vec![selector, naming, version],
    )]);
    let order: Vec<&str> = module_entries(&module)[0]
        .fields
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(order, vec!["naming", "version", "selector"]);
}

#[test]
fn path_types_reach_structure_members_and_fall_back_to_string() {
    let creds = Shape {
        id: "m#Creds".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![crate::ir::Member {
                name: "token".into(),
                target: Tref::Prim(crate::ir::Prim::I32),
                required: true,
                default: None,
                constraints: vec![],
                traits: vec![],
            }],
        },
        traits: vec![],
    };
    let mut structured = field("creds", vec![]);
    structured.target = Tref::Ref {
        id: "m#Creds".into(),
        args: vec![],
    };
    let module = module_of(vec![creds, entry_shape("m#client", vec![structured])]);
    let entries = module_entries(&module);
    let entry = &entries[0];
    let t = entry.path_type(&["creds".into(), "token".into()], &module);
    assert!(matches!(t, Tref::Prim(crate::ir::Prim::I32)));
    // Unresolvable paths read as strings so the emitters stay total.
    let t = entry.path_type(&["creds".into(), "nope".into()], &module);
    assert!(matches!(t, Tref::Prim(crate::ir::Prim::String)));
    let t = entry.path_type(&["ghost".into()], &module);
    assert!(matches!(t, Tref::Prim(crate::ir::Prim::String)));
}

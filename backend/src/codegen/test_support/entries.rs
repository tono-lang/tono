//! The entry-model builders: the shared resolution-matrix module and the
//! mutators each per-target entry test applies to it. Split from the base
//! builders so neither file outgrows the size ceiling.

use super::{enum_shape, enum_values, member, member_constrained, structure};
use crate::ir::{
    Constraint, EnumBacking, Module, Prim, Shape, ShapeKind, TemplatePart, Trait, Tref,
    WireBinding, WireValue,
};

/// An entry module exercising every resolution idiom the entry emitters
/// spell: each env-parsed primitive, guaranteed and why-tracked chains, a
/// dynamic env name, format transforms, both select flavors, `@bind`
/// composition with a member chain, structured and whole-JSON sources with
/// explicit values, and ops covering the descriptor, no-output, primitive
/// output, and bespoke-stub method paths. Shared by the per-target entry
/// tests so their coverage of the shared spelling stays symmetric.
pub fn entries_matrix_module() -> Module {
    use crate::ir::{ArmValue, EntryField, EnvName, FieldRef, Select, SelectArm, Source};
    use serde_json::json;

    fn ef(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
        EntryField {
            name: name.into(),
            target,
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
    let env = |n: &str| Source::Env(EnvName::Name(n.into()));
    let env_ref = |f: &str| {
        Source::Env(EnvName::Field(FieldRef {
            field: vec![f.into()],
        }))
    };

    let mut fields = vec![
        ef("u", Tref::Prim(Prim::Uuid), vec![Source::Arg]),
        ef(
            "flag",
            Tref::Prim(Prim::Bool),
            vec![env("FLAG"), Source::Default(json!(true))],
        ),
        ef("tiny", Tref::Prim(Prim::I8), vec![env("TINY")]),
        ef("small", Tref::Prim(Prim::I16), vec![env("SMALL")]),
        ef("wide", Tref::Prim(Prim::I64), vec![env("WIDE")]),
        ef("utiny", Tref::Prim(Prim::U8), vec![env("UTINY")]),
        ef("uwide", Tref::Prim(Prim::U64), vec![env("UWIDE")]),
        ef(
            "ratio",
            Tref::Prim(Prim::Float),
            vec![env("RATIO"), Source::With],
        ),
        ef("wait", Tref::Prim(Prim::Duration), vec![env("WAIT")]),
        ef(
            "stamp",
            Tref::Prim(Prim::Timestamp),
            vec![env("STAMP"), Source::Default(json!("2020-01-01T00:00:00Z"))],
        ),
        ef(
            "mode",
            Tref::Ref {
                id: "m#mode".into(),
                args: vec![],
            },
            vec![env("MODE"), Source::Default(json!("fast"))],
        ),
        ef("naming", Tref::Prim(Prim::String), vec![env("NAMING")]),
        ef(
            "dynamic",
            Tref::Prim(Prim::String),
            vec![env_ref("naming"), Source::With],
        ),
        ef(
            "sure_name",
            Tref::Prim(Prim::String),
            vec![Source::Default(json!("base"))],
        ),
        ef(
            "dynamic_sure",
            Tref::Prim(Prim::String),
            vec![env_ref("sure_name"), Source::Default(json!("d"))],
        ),
    ];
    let mut derived = ef("derived", Tref::Prim(Prim::String), vec![]);
    derived.format = Some(vec![
        crate::ir::TemplatePart::Lit("k-".into()),
        crate::ir::TemplatePart::Field(vec!["naming".into()]),
        crate::ir::TemplatePart::Field(vec!["tiny".into()]),
        crate::ir::TemplatePart::Input("ignored".into()),
    ]);
    derived.transforms = vec![
        "trim".into(),
        "lower".into(),
        "upper".into(),
        "snake".into(),
        "kebab".into(),
        "pascal".into(),
        "upper_snake".into(),
    ];
    fields.push(derived);
    let mut picked = ef("picked", Tref::Prim(Prim::String), vec![]);
    picked.select = Some(Select {
        subject_index: None,
        subject: vec!["naming".into()],
        arms: vec![
            SelectArm {
                pattern: Some(json!("a")),
                value: ArmValue::Field(vec!["dynamic".into()]),
            },
            SelectArm {
                pattern: Some(json!(1)),
                value: ArmValue::Lit(json!("one")),
            },
            SelectArm {
                pattern: None,
                value: ArmValue::Sources(vec![
                    Source::With,
                    env("PICKED"),
                    Source::Default(json!("p")),
                ]),
            },
        ],
    });
    fields.push(picked);
    let mut sure_pick = ef("sure_pick", Tref::Prim(Prim::String), vec![]);
    sure_pick.select = Some(Select {
        subject_index: None,
        subject: vec!["sure_name".into()],
        arms: vec![SelectArm {
            pattern: Some(json!("base")),
            value: ArmValue::Lit(json!("b")),
        }],
    });
    fields.push(sure_pick);
    let mut composed = ef(
        "composed",
        Tref::Ref {
            id: "m#conf".into(),
            args: vec![],
        },
        vec![],
    );
    composed.binds = vec![
        crate::ir::Bind {
            field: "key".into(),
            source: vec!["naming".into()],
        },
        crate::ir::Bind {
            field: "sure".into(),
            source: vec!["sure_name".into()],
        },
    ];
    fields.push(composed);
    fields.push(ef(
        "creds",
        Tref::Ref {
            id: "m#credentials".into(),
            args: vec![],
        },
        vec![Source::With, env("CREDS")],
    ));
    fields.push(ef(
        "creds_arg",
        Tref::Ref {
            id: "m#credentials".into(),
            args: vec![],
        },
        vec![Source::Arg],
    ));
    fields.push(ef(
        "labels",
        Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::String)),
        ),
        vec![Source::With, env("LABELS")],
    ));
    fields.push(ef(
        "tags",
        Tref::List(Box::new(Tref::Prim(Prim::String))),
        vec![Source::Arg],
    ));

    let op_full = Shape {
        id: "m#api.fetch_note".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            output: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            errors: vec![],
            wire: Some(Box::new(WireBinding {
                method: "GET".into(),
                uri: WireValue::Template(vec![
                    TemplatePart::Lit("/x/".into()),
                    TemplatePart::Field(vec!["naming".into()]),
                ]),
                body: None,
                response_bindings: Default::default(),
                success: vec![200],
                endpoint: Some(WireValue::Field(vec!["naming".into()])),
                request_headers: vec![(
                    vec![TemplatePart::Lit("X-K".into())],
                    WireValue::Field(vec!["derived".into()]),
                )],
                query: vec![],
                timeout: Some(vec!["wait".into()]),
                retry: Some(vec!["tiny".into()]),
            })),
            impl_call: None,
        },
        traits: vec![
            Trait {
                id: "doc".into(),
                value: serde_json::json!(["Fetches."]),
            },
            Trait {
                id: "http".into(),
                value: serde_json::json!({"method": "GET", "path": "/x/{.naming}", "endpoint": {"field": ["naming"]}}),
            },
            Trait {
                id: "header".into(),
                value: serde_json::json!(["X-K", {"field": ["derived"]}]),
            },
            Trait {
                id: "timeout".into(),
                value: serde_json::json!([{"field": ["wait"]}]),
            },
            Trait {
                id: "retry".into(),
                value: serde_json::json!([{"field": ["tiny"]}]),
            },
        ],
    };
    fn bare_wire() -> WireBinding {
        WireBinding {
            method: "GET".into(),
            uri: WireValue::Template(vec![TemplatePart::Lit("/x".into())]),
            body: None,
            response_bindings: Default::default(),
            success: vec![200],
            endpoint: Some(WireValue::Field(vec!["naming".into()])),
            request_headers: Vec::new(),
            query: vec![],
            timeout: None,
            retry: None,
        }
    }
    let op_bare = Shape {
        id: "m#api.ping".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: None,
            errors: vec![],
            wire: Some(Box::new(bare_wire())),
            impl_call: None,
        },
        traits: vec![],
    };
    let op_prim = Shape {
        id: "m#api.count".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: Some(Tref::Prim(Prim::I32)),
            errors: vec![],
            wire: Some(Box::new(bare_wire())),
            impl_call: None,
        },
        traits: vec![],
    };
    let op_stub = Shape {
        id: "m#api.local".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            output: Some(Tref::Prim(Prim::String)),
            errors: vec![],
            wire: None,
            impl_call: None,
        },
        traits: vec![],
    };

    Module {
        tests: vec![],
        name: "m".into(),
        shapes: vec![
            Shape {
                id: "m#mode".into(),
                kind: ShapeKind::Enum {
                    backing: EnumBacking::String,
                    values: enum_values(vec![("fast".into(), None), ("slow".into(), None)]),
                },
                traits: vec![],
            },
            Shape {
                id: "m#conf".into(),
                kind: ShapeKind::Config {
                    fields: vec![
                        ef("key", Tref::Prim(Prim::String), vec![env("KEY")]),
                        ef(
                            "sure",
                            Tref::Prim(Prim::String),
                            vec![env("SURE"), Source::Default(json!("s"))],
                        ),
                        ef("port", Tref::Prim(Prim::I32), vec![env("PORT")]),
                    ],
                },
                traits: vec![],
            },
            structure(
                "m#credentials",
                vec![member("token", Tref::Prim(Prim::String), true)],
            ),
            structure("m#note", vec![member("id", Tref::Prim(Prim::String), true)]),
            Shape {
                id: "m#api".into(),
                kind: ShapeKind::Entry {
                    fields,
                    operations: vec![op_full, op_bare, op_prim, op_stub],
                },
                traits: vec![Trait {
                    id: "doc".into(),
                    value: serde_json::json!(["The matrix entry."]),
                }],
            },
        ],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![],
    }
}

/// A single-name env source chain.
fn env(name: &str) -> Vec<crate::ir::Source> {
    vec![crate::ir::Source::Env(crate::ir::EnvName::Name(
        name.into(),
    ))]
}

/// A bare entry field: only a name, a target, and a source chain. The entry
/// tests build every fixture through this so the wordy IR literal lives once.
pub fn bare_entry_field(
    name: &str,
    target: Tref,
    sources: Vec<crate::ir::Source>,
) -> crate::ir::EntryField {
    crate::ir::EntryField {
        name: name.into(),
        target,
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

/// Push a field onto every entry of the module.
pub fn push_entry_field(module: &mut Module, field: crate::ir::EntryField) {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
            fields.push(field.clone());
        }
    }
}

/// Push a member onto every config shape of the module.
pub fn push_config_member(module: &mut Module, field: crate::ir::EntryField) {
    for shape in &mut module.shapes {
        if let ShapeKind::Config { fields } = &mut shape.kind {
            fields.push(field.clone());
        }
    }
}

/// Attach a protocol trait to every entry operation.
pub fn push_entry_op_trait(module: &mut Module, id: &str, value: serde_json::Value) {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                op.traits.push(Trait {
                    id: id.into(),
                    value: value.clone(),
                });
            }
        }
    }
}

/// Mark every entry operation as wire-bound, for a test that needs
/// `has_wire` to hold (a declared test's `dep: http` stub, an impl-coverage
/// check) without caring about any specific binding field.
pub fn push_entry_op_wire(module: &mut Module, method: &str) {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                if let ShapeKind::Operation { wire, .. } = &mut op.kind {
                    *wire = Some(super::wire_binding(method));
                }
            }
        }
    }
}

/// Set every entry operation's output type.
pub fn set_entry_op_outputs(module: &mut Module, target: Tref) {
    for shape in &mut module.shapes {
        if let ShapeKind::Entry { operations, .. } = &mut shape.kind {
            for op in operations {
                if let ShapeKind::Operation { output, .. } = &mut op.kind {
                    *output = Some(target.clone());
                }
            }
        }
    }
}

/// The wire struct with a required checked member the strict structured
/// decode tests share.
pub fn credentials_shape(id: &str) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![
                member_constrained(
                    "token",
                    Tref::Prim(Prim::String),
                    vec![Constraint::Length {
                        min: Some(1),
                        max: None,
                    }],
                ),
                member("account_id", Tref::Prim(Prim::String), true),
            ],
        },
        traits: vec![],
    }
}

/// A `@str::*` pipeline on a plain chain field: transforms must apply to the
/// resolved value even without a `@format`.
pub fn with_transformed_chain_field(module: &mut Module) {
    let mut team = bare_entry_field("team", Tref::Prim(Prim::String), env("TEAM"));
    team.transforms = vec!["snake".into()];
    push_entry_field(module, team);
}

/// Config members carrying their own derivations: a `@format` with a
/// transform, and a match on a guaranteed sibling.
pub fn with_derived_config_members(module: &mut Module) {
    let mut label = bare_entry_field("label", Tref::Prim(Prim::String), vec![]);
    label.format = Some(vec![
        crate::ir::TemplatePart::Lit("conf-".into()),
        crate::ir::TemplatePart::Field(vec!["client_name".into()]),
    ]);
    label.transforms = vec!["upper".into()];
    push_config_member(module, label);
    let mut size = bare_entry_field("size", Tref::Prim(Prim::String), vec![]);
    size.select = Some(crate::ir::Select {
        subject_index: None,
        subject: vec!["client_name".into()],
        arms: vec![crate::ir::SelectArm {
            pattern: Some(serde_json::json!("demo")),
            value: crate::ir::ArmValue::Lit(serde_json::json!("small")),
        }],
    });
    push_config_member(module, size);
}

/// A config member matching on a why-tracked sibling, with one arm reading
/// another absent chain and one inline-source arm.
pub fn with_member_select_on_absent_subject(module: &mut Module) {
    let mut zone = bare_entry_field("zone", Tref::Prim(Prim::String), vec![]);
    zone.select = Some(crate::ir::Select {
        subject_index: None,
        subject: vec!["endpoint".into()],
        arms: vec![
            crate::ir::SelectArm {
                pattern: Some(serde_json::json!("https://api.example.com")),
                value: crate::ir::ArmValue::Field(vec!["endpoint_v1".into()]),
            },
            crate::ir::SelectArm {
                pattern: None,
                value: crate::ir::ArmValue::Sources(env("ZONE")),
            },
        ],
    });
    push_config_member(module, zone);
}

/// A consumed bytes head plus a range-constrained numeric chain: the requires
/// and the constraint presence gate for the non-string scalars.
pub fn with_bytes_and_constrained_port(module: &mut Module) {
    push_entry_field(
        module,
        bare_entry_field("secret", Tref::Prim(Prim::Bytes), env("SECRET")),
    );
    let mut port = bare_entry_field("port", Tref::Prim(Prim::I32), env("PORT"));
    port.constraints = vec![Constraint::Range {
        min: Some(1.0),
        max: None,
        excl_min: false,
        excl_max: false,
    }];
    push_entry_field(module, port);
    push_entry_op_trait(
        module,
        "header",
        serde_json::json!(["X-Secret", {"field": ["secret"]}]),
    );
}

/// An enum-typed config member: it must freeze into the runtime values as a
/// branded string like a top-level enum field.
pub fn with_enum_config_member(module: &mut Module) {
    module.shapes.push(enum_shape(
        "notes#Mode",
        vec![("live".into(), None), ("test".into(), None)],
    ));
    push_config_member(
        module,
        bare_entry_field(
            "mode",
            Tref::Ref {
                id: "notes#Mode".into(),
                args: vec![],
            },
            env("MODE"),
        ),
    );
}

/// The structured and whole-JSON sources the strict decode tests share: a
/// credentials struct decoded from an env variable, and a labels map decoded
/// whole.
pub fn with_structured_sources(module: &mut Module, creds_sources: Vec<crate::ir::Source>) {
    module.shapes.push(credentials_shape("notes#credentials"));
    push_entry_field(
        module,
        bare_entry_field(
            "creds",
            Tref::Ref {
                id: "notes#credentials".into(),
                args: vec![],
            },
            creds_sources,
        ),
    );
    push_entry_field(
        module,
        bare_entry_field(
            "labels",
            Tref::Map(
                Box::new(Tref::Prim(Prim::String)),
                Box::new(Tref::Prim(Prim::String)),
            ),
            env("SERVICE_LABELS"),
        ),
    );
}

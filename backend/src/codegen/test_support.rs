//! Shared builders and assertions for codegen unit tests, so every target's
//! tests construct IR shapes and check their symbol table the same way instead
//! of re-declaring the same helpers.
#![cfg(test)]

use crate::codegen::symbol::Symbol;
use crate::codegen::target::{RenderRules, Target};
use crate::codegen::tree::Decl;
use crate::ir::{
    Constraint, EnumBacking, EnumValue, Member, Module, Prim, Shape, ShapeKind, Trait, Tref,
};

/// Convert `(wire, discriminant)` test tuples into bagless enum values.
fn enum_values(values: Vec<(String, Option<i64>)>) -> Vec<EnumValue> {
    values
        .into_iter()
        .map(|(name, value)| EnumValue {
            name,
            value,
            traits: vec![],
        })
        .collect()
}

/// A required member with no traits.
pub fn member(name: &str, target: Tref, required: bool) -> Member {
    member_with(name, target, required, vec![])
}

/// A required member carrying validation constraints, used by the validator tests.
pub fn member_constrained(name: &str, target: Tref, constraints: Vec<Constraint>) -> Member {
    Member {
        name: name.into(),
        target,
        required: true,
        default: None,
        constraints,
        traits: vec![],
    }
}

/// A member with explicit traits.
pub fn member_with(name: &str, target: Tref, required: bool, traits: Vec<Trait>) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits,
    }
}

/// A union member: a named arm whose payload is a nominal reference, with an
/// optional `@wire` tag override. Used by the union-codec tests in every target.
pub fn wire_member(name: &str, payload_id: &str, wire: Option<&str>) -> Member {
    let traits = wire
        .map(|w| {
            vec![Trait {
                id: "core#wire".into(),
                value: serde_json::json!(w),
            }]
        })
        .unwrap_or_default();
    member_with(
        name,
        Tref::Ref {
            id: payload_id.into(),
            args: vec![],
        },
        true,
        traits,
    )
}

/// A structure shape with the given members.
pub fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

/// A string-backed enum shape with the given `(wire, discriminant)` values.
pub fn enum_shape(id: &str, values: Vec<(String, Option<i64>)>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Enum {
            backing: EnumBacking::String,
            values: enum_values(values),
        },
        traits: vec![],
    }
}

/// An int-backed enum shape with the given `(wire, discriminant)` values.
pub fn int_enum_shape(id: &str, values: Vec<(String, Option<i64>)>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Enum {
            backing: EnumBacking::Int,
            values: enum_values(values),
        },
        traits: vec![],
    }
}

/// A union shape with the given discriminator and variant members.
pub fn union_shape(id: &str, discriminator: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Union {
            params: vec![],
            discriminator: discriminator.into(),
            members,
        },
        traits: vec![],
    }
}

/// A bare trait with the given id and JSON value.
pub fn trait_of(id: &str, value: serde_json::Value) -> Trait {
    Trait {
        id: id.into(),
        value,
    }
}

/// An error shape carrying its discrimination traits: the HTTP status, an
/// optional body code, and retryability.
pub fn error_shape(
    id: &str,
    members: Vec<Member>,
    status: i64,
    code: Option<&str>,
    retryable: bool,
) -> Shape {
    let mut shape = structure(id, members);
    shape
        .traits
        .push(trait_of("status", serde_json::json!([status])));
    if let Some(code) = code {
        shape
            .traits
            .push(trait_of("errorCode", serde_json::json!([code])));
    }
    if retryable {
        shape
            .traits
            .push(trait_of("retryable", serde_json::Value::Null));
    }
    shape
}

/// An operation from `m#charge_input` to `m#charge` with the given traits and
/// declared-error references.
pub fn operation(id: &str, traits: Vec<Trait>, errors: Vec<&str>) -> Shape {
    let reference = |id: &str| Tref::Ref {
        id: id.into(),
        args: vec![],
    };
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input: Some(reference("m#charge_input")),
            output: Some(reference("m#charge")),
            errors: errors.into_iter().map(reference).collect(),
        },
        traits,
    }
}

/// The shared error-surface fixture: one async transport operation declaring a
/// retryable, coded 402 error and a codeless 429 error, so every target's
/// tests exercise the same taxonomy, client, and discrimination inputs.
pub fn error_demo_module() -> Module {
    Module {
        name: "m".into(),
        shapes: vec![
            structure(
                "m#charge",
                vec![member("id", Tref::Prim(Prim::String), true)],
            ),
            structure(
                "m#charge_input",
                vec![member("amount", Tref::Prim(Prim::I64), true)],
            ),
            error_shape(
                "m#payment_declined",
                vec![member("message", Tref::Prim(Prim::String), true)],
                402,
                Some("payment_declined"),
                true,
            ),
            error_shape("m#rate_limited", vec![], 429, None, false),
        ],
        operations: vec![operation(
            "m#create_charge",
            vec![trait_of(
                "http",
                serde_json::json!({"method": "POST", "path": "/charges"}),
            )],
            vec!["m#payment_declined", "m#rate_limited"],
        )],
        extensions: vec![],
    }
}

/// Render declarations through a target's render rules, joined by newlines.
pub fn rendered(decls: &[Decl], rules: &impl RenderRules) -> String {
    decls
        .iter()
        .map(|d| rules.render_decl(d))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Assert a symbol table maps each primitive to the expected in-code name and
/// imports none of them.
pub fn assert_prim_symbols(symbol_of: impl Fn(&Tref) -> Symbol, cases: &[(Prim, &str)]) {
    for (prim, expected) in cases {
        let symbol = symbol_of(&Tref::Prim(prim.clone()));
        assert_eq!(&symbol.name, expected, "{prim:?}");
        assert_eq!(
            symbol.import, None,
            "primitives are not imported ({prim:?})"
        );
    }
}

/// Assert a type parameter is an unimported local name and the collection
/// fallbacks carry the given structural names.
pub fn assert_param_and_collections(
    symbol_of: impl Fn(&Tref) -> Symbol,
    list_name: &str,
    map_name: &str,
) {
    let param = symbol_of(&Tref::Param("T".into()));
    assert_eq!(param.name, "T");
    assert_eq!(param.import, None);
    assert_eq!(
        symbol_of(&Tref::List(Box::new(Tref::Prim(Prim::Bool)))).name,
        list_name
    );
    assert_eq!(
        symbol_of(&Tref::Map(
            Box::new(Tref::Prim(Prim::String)),
            Box::new(Tref::Prim(Prim::Bool)),
        ))
        .name,
        map_name
    );
}

/// Assert a target emits nothing for an operation stub and ignores the opaque
/// wire descriptor.
pub fn assert_emits_no_op_stub(target: &impl Target) {
    let op = Shape {
        id: "billing#Create".into(),
        kind: ShapeKind::Operation {
            input: None,
            output: None,
            errors: vec![],
        },
        traits: vec![],
    };
    assert!(target
        .emit_op_stub(&op, &serde_json::json!({"http_method": "POST"}))
        .is_empty());
}

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

    let descriptor = Trait {
        id: "wire_descriptor".into(),
        value: serde_json::json!({"http_method": "GET", "uri": "/x", "endpoint": ["naming"]}),
    };
    let op_full = Shape {
        id: "m#api.fetch_note".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            output: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            errors: vec![],
        },
        traits: vec![
            descriptor.clone(),
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
    let op_bare = Shape {
        id: "m#api.ping".into(),
        kind: ShapeKind::Operation {
            input: None,
            output: None,
            errors: vec![],
        },
        traits: vec![descriptor.clone()],
    };
    let op_prim = Shape {
        id: "m#api.count".into(),
        kind: ShapeKind::Operation {
            input: None,
            output: Some(Tref::Prim(Prim::I32)),
            errors: vec![],
        },
        traits: vec![descriptor],
    };
    let op_stub = Shape {
        id: "m#api.local".into(),
        kind: ShapeKind::Operation {
            input: Some(Tref::Ref {
                id: "m#note".into(),
                args: vec![],
            }),
            output: Some(Tref::Prim(Prim::String)),
            errors: vec![],
        },
        traits: vec![],
    };

    Module {
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
    }
}

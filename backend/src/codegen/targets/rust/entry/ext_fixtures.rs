//! The IR model the round-trip test (`tests/rust_ext_roundtrip.rs`) compiles
//! against a real stand-in crate under `codegen-tests/rust-ext/fixtures/`:
//! the RFC appendix's `companyconfig` library (a config load with a
//! per-field `yields`/`returns` projection and a `match`), scoped to the
//! extern-call field source this target emits. The appendix's injectable
//! `companybus` handle is not included: Rust does not emit a foreign
//! opaque-handle type yet (`TargetKind::emits_ext_handle_types`), so a
//! model exercising it would not validate for this target.

use crate::ir::{
    ArmValue, CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam,
    ForeignField, ForeignStruct, LangPath, Member, Model, Module, Prim, ReturnsField, ReturnsLit,
    ReturnsValue, Select, SelectArm, Shape, ShapeKind, Source, Tref, YieldsPos, TONO_IR_VERSION,
};

fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| (*s).to_string()).collect()
}

fn ref_to(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
}

fn string_field(name: &str, sources: Vec<Source>) -> EntryField {
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

fn member(name: &str) -> Member {
    Member {
        name: name.into(),
        target: Tref::Prim(Prim::String),
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

/// A foreign struct whose fields are all `string`, save one that may point
/// at another declared foreign struct (`nested`).
fn foreign_struct(
    name: &str,
    string_fields: &[&str],
    nested: Option<(&str, &str)>,
) -> ForeignStruct {
    let mut fields: Vec<ForeignField> = string_fields
        .iter()
        .map(|f| ForeignField {
            name: (*f).to_string(),
            r#type: Tref::Prim(Prim::String),
        })
        .collect();
    if let Some((field, target_id)) = nested {
        fields.push(ForeignField {
            name: field.into(),
            r#type: ref_to(target_id),
        });
    }
    ForeignStruct {
        name: name.into(),
        fields,
    }
}

fn string_params(names: &[&str]) -> Vec<ExternParam> {
    names
        .iter()
        .map(|n| ExternParam {
            name: (*n).to_string(),
            r#type: Tref::Prim(Prim::String),
        })
        .collect()
}

fn ref_args(names: &[&str]) -> Vec<CallArg> {
    names
        .iter()
        .map(|n| CallArg::Ref(vec![(*n).to_string()]))
        .collect()
}

fn field_path(segments: &[&str]) -> ArmValue {
    ArmValue::Field(strings(segments))
}

/// The RFC appendix's `companyconfig.load`, scoped to the extern-call
/// field source: an entry `config` field reads `service`/`region`, the
/// library returns a shape whose `Env` is matched to pick `Host`/`DevHost`,
/// and `token` reads `Credentials.Secret`.
pub fn rust_ext_fixture_model() -> Model {
    let service = string_field("service", vec![Source::Default(serde_json::json!("notes"))]);
    let region = string_field("region", vec![Source::Arg]);
    let mut config = string_field("config", vec![]);
    config.target = ref_to("m#app_config");
    config.call = Some(EntryCall {
        ns: "companyconfig".into(),
        func: "load".into(),
        args: ref_args(&["service", "region"]),
    });

    let app_config = Shape {
        id: "m#app_config".into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: vec![member("endpoint"), member("token")],
        },
        traits: vec![],
    };

    let client = Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![service, region, config],
            operations: vec![],
        },
        traits: vec![],
    };

    let ext_lib = ExtLib {
        name: "companyconfig".into(),
        langs: vec![LangPath {
            lang: "rust".into(),
            path: "companyconfig".into(),
        }],
        structs: vec![
            foreign_struct("Creds", &["secret"], None),
            foreign_struct(
                "Config",
                &["host", "dev_host", "env"],
                Some(("credentials", "companyconfig#Creds")),
            ),
        ],
        types: vec![],
        externs: vec![ExternDecl {
            name: "load".into(),
            params: string_params(&["service", "region"]),
            r#return: ref_to("m#app_config"),
            langs: vec![ExternLang {
                lang: "rust".into(),
                symbol: "load".into(),
                call_args: ref_args(&["service", "region"]),
                yields: vec![YieldsPos {
                    name: "cfg".into(),
                    r#type: Some(ref_to("companyconfig#Config")),
                    is_error: false,
                }],
                returns: Some(ReturnsLit {
                    r#type: ref_to("m#app_config"),
                    fields: vec![
                        ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Select(Select {
                                subject: strings(&["cfg", "env"]),
                                subject_index: None,
                                arms: vec![
                                    SelectArm {
                                        pattern: Some(serde_json::json!("prod")),
                                        value: field_path(&["cfg", "host"]),
                                    },
                                    SelectArm {
                                        pattern: None,
                                        value: field_path(&["cfg", "dev_host"]),
                                    },
                                ],
                            }),
                        },
                        ReturnsField {
                            name: "token".into(),
                            value: ReturnsValue::Field(strings(&["cfg", "credentials", "secret"])),
                        },
                    ],
                }),
                errors: vec![],
                sync: false,
                infallible: false,
            }],
        }],
    };

    let module = Module {
        name: "m".into(),
        shapes: vec![client, app_config],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![ext_lib],
        tests: vec![],
    };
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![module],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::pipeline::generate_target;
    use crate::codegen::targets::rust::types::rust_casing;
    use crate::codegen::{CodegenConfig, TargetKind};

    /// `tests/rust_ext_roundtrip.rs` (the real verification: the fixture
    /// crate compiling the emitted call) skips under `cargo-llvm-cov`, since
    /// coverage instrumentation and a real `cargo build` subprocess don't
    /// mix; this proves the model itself builds a well-formed IR that
    /// generation accepts, so the fixture stays covered by the normal
    /// `cargo test --lib` run.
    #[test]
    fn the_fixture_model_generates_without_error() {
        let model = rust_ext_fixture_model();
        let files = generate_target(
            &model,
            TargetKind::Rust,
            &CodegenConfig::default(),
            &rust_casing(),
        )
        .expect("the fixture model must generate cleanly");
        assert!(!files.is_empty());
    }
}

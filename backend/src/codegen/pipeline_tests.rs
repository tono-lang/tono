//! The pipeline's tests, in a file of their own so the module stays inside
//! the source-size ceiling.

use super::*;
use crate::codegen::layout::check_layout;
use crate::codegen::test_support::{member, structure, union_shape, wire_binding};
use crate::ir::{
    EntryField, Module, Prim, Shape, ShapeKind, Source, TemplatePart, Tref, WireBinding, WireValue,
};
use std::path::PathBuf;

/// A model whose Go module carries a union, so Go splits into two files while
/// Rust and TypeScript stay single-file.
fn union_model() -> Model {
    Model {
        tono_ir_version: 3,
        modules: vec![Module {
            tests: vec![],
            name: "payments".into(),
            shapes: vec![
                structure(
                    "payments#Account",
                    vec![member(
                        "method",
                        Tref::Ref {
                            id: "payments#Method".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                union_shape(
                    "payments#Method",
                    "type",
                    vec![member(
                        "card",
                        Tref::Ref {
                            id: "payments#Card".into(),
                            args: vec![],
                        },
                        true,
                    )],
                ),
                structure(
                    "payments#Card",
                    vec![member("last4", Tref::Prim(Prim::String), true)],
                ),
            ],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    }
}

fn demo_model() -> Model {
    Model {
        tono_ir_version: 3,
        modules: vec![Module {
            tests: vec![],
            name: "payments".into(),
            shapes: vec![structure(
                "payments#Charge",
                vec![member("amount", Tref::Prim(Prim::I64), true)],
            )],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    }
}

#[test]
fn rust_folds_a_modules_internal_group_into_its_public_file() {
    // Rust fences with visibility, not with a location, so a declaration no
    // public type reaches stays in the module's own file and says
    // `pub(crate)`. No file is named for its audience, and the module tree
    // still declares and re-exports exactly one module.
    let mut model = demo_model();
    model.modules[0].shapes[0]
        .traits
        .push(crate::codegen::test_support::trait_of(
            "pub",
            serde_json::Value::Null,
        ));
    // A shape no public declaration reaches: the module's own business.
    model.modules[0].shapes.push(structure(
        "payments#Ledger",
        vec![member("entries", Tref::Prim(Prim::String), true)],
    ));
    let files = generate(&model, &[TargetKind::Rust], &CodegenConfig::default())
        .expect("generate a Rust SDK");
    let paths: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    assert!(
        !paths.iter().any(|p| p.contains("internal")),
        "no Rust file is named for its audience, got {paths:?}"
    );
    let types = text_at(&files, "rust/payments/types.rs");
    assert!(types.contains("pub struct Charge {"));
    // The ledger is reached from nothing public, so it is the module's own
    // business and rides the same file, out of reach.
    assert!(types.contains("pub(crate) struct Ledger {"));
    assert!(types.contains("#[allow(dead_code)]"));
    assert!(text_at(&files, "rust/payments/mod.rs").contains("pub mod types;"));
}

#[test]
fn generate_splits_each_target_that_has_serialization_to_emit() {
    let model = demo_model();
    let files = generate(
        &model,
        &[TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
        &CodegenConfig::default(),
    )
    .unwrap();
    let paths: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    let sep = std::path::MAIN_SEPARATOR;
    // Each module is a directory of groups. The i64 field routes through a
    // helper module and pulls TypeScript codecs, both of which land in the
    // internal groups; Go's plain tagged struct needs no serde of its own, so
    // that module emits only its public group.
    assert_eq!(
        paths,
        vec![
            format!("rust{sep}number.rs"),
            format!("rust{sep}payments{sep}types.rs"),
            format!("rust{sep}lib.rs"),
            format!("rust{sep}payments{sep}mod.rs"),
            format!("go{sep}payments{sep}types.go"),
            format!("typescript{sep}number.ts"),
            format!("typescript{sep}payments{sep}types.ts"),
            format!("typescript{sep}payments{sep}codec.ts"),
            format!("typescript{sep}payments{sep}index.ts"),
            format!("typescript{sep}package.json"),
        ]
    );
    // Every source file carries the banner (the JSON manifest cannot); each
    // target spells the struct its own way in its public group, and Go carries
    // its package clause.
    assert!(files
        .iter()
        .filter(|f| f.path.extension().is_some_and(|e| e != "json"))
        .all(|f| f.text.starts_with(BANNER)));
    assert!(text_at(&files, "rust/number.rs").contains("pub mod i64_string"));
    // One module shares nothing, so the branded well-known types would ride
    // its public group rather than a group of their own; no field names one,
    // so none is emitted at all.
    assert!(!text_at(&files, "rust/payments/types.rs").contains("pub struct Timestamp"));
    assert!(text_at(&files, "rust/payments/types.rs").contains("pub struct Charge"));
    assert!(text_at(&files, "go/payments/types.go").contains("package payments"));
    assert!(text_at(&files, "go/payments/types.go").contains("type Charge struct"));
    assert!(text_at(&files, "typescript/payments/types.ts").contains("export interface Charge"));
    let ts_internal = text_at(&files, "typescript/payments/codec.ts");
    assert!(ts_internal.contains("export function encodeCharge"));
    assert!(ts_internal.contains("import { Charge } from \"./types\";"));
}

#[test]
fn a_union_module_splits_go_and_typescript_but_rusts_derive_keeps_it_single() {
    let files = generate(
        &union_model(),
        &[TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
        &CodegenConfig::default(),
    )
    .unwrap();
    let paths: Vec<String> = files
        .iter()
        .map(|f| f.path.to_string_lossy().into_owned())
        .collect();
    // Go needs hand-written union marshaling and TypeScript needs codecs, so
    // both get an internal group. Rust's tagged-union enum derives its serde on
    // the type with no wide integer, bytes, or open enum in the module, so
    // there is nothing internal for it to hold.
    assert!(paths.contains(&"rust/payments/types.rs".replace('/', std::path::MAIN_SEPARATOR_STR)));
    assert!(!paths.contains(&"rust/payments/codec.rs".replace('/', std::path::MAIN_SEPARATOR_STR)));
    let go_types = text_at(&files, "go/payments/types.go");
    let go_internal = text_at(&files, "go/payments/codec.go");
    // Both groups keep their banner and the module's package clause; the split
    // puts the interface in the public group and the serialization in the
    // internal one, and the two are one Go package, so neither imports the
    // other.
    assert!(go_types.starts_with(BANNER));
    assert!(go_types.contains("package payments"));
    assert!(go_types.contains("type Method interface{ isMethod() }"));
    assert!(!go_types.contains("import "));
    assert!(!go_types.contains("MarshalJSON"));

    assert!(go_internal.starts_with(BANNER));
    assert!(go_internal.contains("package payments"));
    assert!(go_internal.contains("func marshalVariant("));
    assert!(go_internal.contains("func UnmarshalMethod(b []byte) (Method, error) {"));
    assert!(go_internal.contains("func (a *Account) UnmarshalJSON(b []byte) error {"));
    assert!(go_internal.contains("\"encoding/json\""));
    assert!(go_internal.contains("\"fmt\""));
}

/// A model with one loose, bespoke (non-wire) async operation declaring one
/// error, so every target emits the client interface, the error surface, and
/// the discriminator around it. A loose operation is a trait/interface
/// surface only in every target (generation rejects a wire-bound loose
/// operation outright), so this fixture carries no `@http`/wire binding at
/// all: `@async` alone keeps its effect async, and the declared error alone
/// keeps the `Api` category and the discriminator live.
fn ops_model() -> Model {
    let mut not_found = structure(
        "payments#not_found",
        vec![member("message", Tref::Prim(Prim::String), true)],
    );
    not_found.traits = vec![crate::ir::Trait {
        id: "status".into(),
        value: serde_json::json!([404]),
    }];
    let mut model = demo_model();
    model.modules[0].shapes.push(not_found);
    model.modules[0].operations = vec![Shape {
        id: "payments#get_charge".into(),
        kind: ShapeKind::Operation {
            input_name: None,
            input: None,
            output: Some(Tref::Ref {
                id: "payments#Charge".into(),
                args: vec![],
            }),
            errors: vec![Tref::Ref {
                id: "payments#not_found".into(),
                args: vec![],
            }],
            wire: None,
            impl_call: None,
        },
        traits: vec![crate::ir::Trait {
            id: "async".into(),
            value: serde_json::Value::Null,
        }],
    }];
    model
}

#[test]
fn a_module_with_operations_generates_the_error_surface_in_every_target() {
    let files = generate(
        &ops_model(),
        &[TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
        &CodegenConfig::default(),
    )
    .unwrap();
    let text_of = |path: &str| text_at(&files, path).to_string();

    // Rust: the enum root, the Api payload enum, the async client trait, and
    // the discriminator in the internal group.
    let rust_types = text_of("rust/payments/types.rs");
    assert!(rust_types.contains("pub enum TonoError {"));
    assert!(rust_types.contains("Undeclared(APIError),"));
    assert!(rust_types.contains("async fn get_charge(&self) -> Result<Charge, TonoError>;"));
    // Rust has no generated client, so the error discriminator is part of the
    // surface a consumer implements the trait against, not something the SDK
    // keeps to itself.
    assert!(rust_types
        .contains("pub fn decode_get_charge_error(status: u16, body: &str) -> TonoError {"));

    // Go: error values with no root, the blocking interface, and the
    // discriminator in the serde file.
    let go_types = text_of("go/payments/types.go");
    assert!(go_types.contains("type APIError struct {"));
    assert!(!go_types.contains("TonoError"));
    assert!(go_types.contains("GetCharge() (Charge, error)"));
    assert!(go_types.contains("func (e *NotFound) Retryable() bool { return false }"));
    let go_serde = text_of("go/payments/codec.go");
    assert!(go_serde.contains("func DecodeGetChargeError(status int, body []byte) error {"));

    // TypeScript: the class hierarchy, the Promise-returning client, and the
    // discriminator with the codecs.
    let ts_types = text_of("typescript/payments/types.ts");
    assert!(ts_types.contains("export abstract class TonoError extends Error {"));
    assert!(ts_types.contains("export class NotFoundError extends APIError {"));
    assert!(ts_types.contains("getCharge(): Promise<Charge>;"));
    let ts_serde = text_of("typescript/payments/codec.ts");
    assert!(ts_serde.contains(
        "export function decodeGetChargeError(status: number, body: string): TonoError {"
    ));
    // No transport happens for a bespoke (non-wire) loose operation, so
    // Client/DecodeError/TransportError stay out: only the categories the
    // declared error and the discriminator actually construct are imported.
    assert!(ts_serde.contains(
        "import { APIError, Charge, NotFound, NotFoundError, STATUS_NOT_FOUND, TonoError } from \"./types\";"
    ));
}

#[test]
fn two_groups_mapping_to_one_path_is_a_defect_not_a_silent_overwrite() {
    let file = |path: &str| GeneratedFile {
        target: TargetKind::Go,
        path: PathBuf::from(path),
        text: String::new(),
    };
    assert!(reject_duplicate_paths(&[file("a.go"), file("b.go")]).is_ok());
    let err = reject_duplicate_paths(&[file("a.go"), file("a.go")]).unwrap_err();
    assert!(err.contains("a.go"));
}

#[test]
fn a_slot_with_no_reference_behind_it_is_a_defect() {
    let file = |text: &str| GeneratedFile {
        target: TargetKind::Go,
        path: PathBuf::from("a.go"),
        text: text.into(),
    };
    assert!(reject_unfilled_slots(&[file("func F() {}")]).is_ok());
    let err = reject_unfilled_slots(&[file(&crate::codegen::tree::symbol_slot("F"))]).unwrap_err();
    assert!(err.contains("a.go"));
}

#[test]
fn generate_with_no_targets_is_empty() {
    let files = generate(&demo_model(), &[], &CodegenConfig::default()).unwrap();
    assert!(files.is_empty());
}

#[test]
fn parse_targets_accepts_a_list_and_rejects_unknowns() {
    assert_eq!(
        parse_targets("rust, go ,ts").unwrap(),
        vec![TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript]
    );
    assert!(parse_targets("rust,java").is_err());
}

// ── Sub-package mapping and config hooks ────────────────────────────

/// A two-module project: `payments.common` defines a type, `payments.charge`
/// references it across the module boundary, exercising the dotted-module ->
/// sub-package mapping and the cross-package import.
fn sub_package_model() -> Model {
    Model {
        tono_ir_version: 2,
        modules: vec![
            Module {
                tests: vec![],
                name: "payments.common".into(),
                shapes: vec![structure(
                    "payments.common#Money",
                    vec![member("amount", Tref::Prim(Prim::I64), true)],
                )],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
            Module {
                tests: vec![],
                name: "payments.charge".into(),
                shapes: vec![structure(
                    "payments.charge#Charge",
                    vec![member(
                        "total",
                        Tref::Ref {
                            id: "payments.common#Money".into(),
                            args: vec![],
                        },
                        true,
                    )],
                )],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
        ],
    }
}

fn paths_of(files: &[GeneratedFile]) -> Vec<String> {
    files
        .iter()
        .map(|f| {
            f.path
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/")
        })
        .collect()
}

fn text_at<'a>(files: &'a [GeneratedFile], path: &str) -> &'a str {
    let sep = std::path::MAIN_SEPARATOR_STR;
    let want = path.replace('/', sep);
    &files
        .iter()
        .find(|f| f.path.to_string_lossy() == want)
        .unwrap_or_else(|| panic!("no file at {path}"))
        .text
}

#[test]
fn dotted_modules_map_to_idiomatic_sub_packages() {
    let files = generate(
        &sub_package_model(),
        &[TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript],
        &CodegenConfig::default(),
    )
    .unwrap();
    let paths = paths_of(&files);
    // Rust and TypeScript use the dotted path as a file path; Go nests the
    // file inside a package directory named for the last segment.
    assert!(paths.contains(&"rust/payments/common/types.rs".to_string()));
    assert!(paths.contains(&"rust/payments/charge/types.rs".to_string()));
    assert!(paths.contains(&"go/payments/common/types.go".to_string()));
    assert!(paths.contains(&"typescript/payments/common/types.ts".to_string()));
    assert!(paths.contains(&"typescript/payments/charge/types.ts".to_string()));

    // The cross-package reference imports through each language's idiomatic
    // module path: Rust an absolute crate path and TypeScript a path relative
    // to the importing file. The Go cross-package import needs the module path
    // and is covered by [`go_module_prefix_makes_cross_package_imports_absolute`];
    // emitting it without a module path is rejected by [`check_layout`].
    assert!(text_at(&files, "rust/payments/charge/types.rs")
        .contains("use crate::payments::common::types::Money;"));
    assert!(
        text_at(&files, "typescript/payments/charge/types.ts").contains("from \"../common/types\"")
    );
    // The Go package is named for the last segment, not the dotted path.
    assert!(text_at(&files, "go/payments/common/types.go").contains("package common"));

    // Rust gets a module tree so the crate paths resolve, and each module
    // re-exports its public groups.
    assert!(paths.contains(&"rust/payments/mod.rs".to_string()));
    let namespace = text_at(&files, "rust/payments/mod.rs");
    assert!(namespace.contains("pub mod common;"));
    assert!(namespace.contains("pub mod charge;"));
    assert!(text_at(&files, "rust/payments/common/mod.rs").contains("pub use types::*;"));
    // TypeScript gets a barrel per module, and the package's exports map lists
    // exactly those.
    assert!(text_at(&files, "typescript/payments/common/index.ts").contains("from \"./types\";"));
    let manifest = text_at(&files, "typescript/package.json");
    assert!(manifest.contains("\"./payments/common\": \"./payments/common/index.ts\""));
    assert!(!manifest.contains("internal"));
}

#[test]
fn go_module_prefix_makes_cross_package_imports_absolute() {
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    let files = generate(&sub_package_model(), &[TargetKind::Go], &config).unwrap();
    assert!(text_at(&files, "go/payments/charge/types.go")
        .contains("import \"example.com/sdk/payments/common\""));
}

#[test]
fn flatten_collapses_modules_into_flat_packages() {
    let config = CodegenConfig {
        flatten: true,
        remap: vec![],
        go_module: None,
    };
    let files = generate(&sub_package_model(), &[TargetKind::Rust], &config).unwrap();
    let paths = paths_of(&files);
    assert!(paths.contains(&"rust/payments_common/types.rs".to_string()));
    assert!(paths.contains(&"rust/payments_charge/types.rs".to_string()));
    assert!(text_at(&files, "rust/payments_charge/types.rs")
        .contains("use crate::payments_common::types::Money;"));
}

#[test]
fn remap_rewrites_the_module_prefix_in_paths_and_imports() {
    let config = CodegenConfig {
        flatten: false,
        remap: vec![("payments".into(), "billing".into())],
        go_module: None,
    };
    let files = generate(&sub_package_model(), &[TargetKind::Rust], &config).unwrap();
    let paths = paths_of(&files);
    assert!(paths.contains(&"rust/billing/common/types.rs".to_string()));
    assert!(paths.contains(&"rust/billing/charge/types.rs".to_string()));
    assert!(text_at(&files, "rust/billing/charge/types.rs")
        .contains("use crate::billing::common::types::Money;"));
}

#[test]
fn a_single_segment_module_still_gets_its_own_package_directory() {
    // A module is a directory of groups in every target, so even the flat
    // single-module case nests: the groups need somewhere to sit together.
    let files = generate(&demo_model(), &[TargetKind::Go], &CodegenConfig::default()).unwrap();
    assert_eq!(paths_of(&files), vec!["go/payments/types.go".to_string()]);
}

#[test]
fn go_multi_module_without_a_module_path_is_rejected() {
    // Multi-module Go with no module path would emit unresolvable bare imports.
    let err = check_layout(
        &sub_package_model(),
        &[TargetKind::Go],
        &CodegenConfig::default(),
    )
    .unwrap_err();
    assert!(err.contains("--go-module"));
    // The module path makes the cross-package imports resolve.
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    assert!(check_layout(&sub_package_model(), &[TargetKind::Go], &config).is_ok());
    // Rust and TypeScript have relative imports, so the same layout is fine.
    assert!(check_layout(
        &sub_package_model(),
        &[TargetKind::Rust, TargetKind::TypeScript],
        &CodegenConfig::default()
    )
    .is_ok());
}

#[test]
fn go_modules_sharing_a_last_segment_are_rejected() {
    // `a.common` and `b.common` both map to package `common` and would collide.
    let model = Model {
        tono_ir_version: 2,
        modules: vec![
            Module {
                tests: vec![],
                name: "a.common".into(),
                shapes: vec![structure(
                    "a.common#Money",
                    vec![member("amount", Tref::Prim(Prim::I64), true)],
                )],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
            Module {
                tests: vec![],
                name: "b.common".into(),
                shapes: vec![structure(
                    "b.common#Rate",
                    vec![member("pct", Tref::Prim(Prim::I64), true)],
                )],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
        ],
    };
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    let err = check_layout(&model, &[TargetKind::Go], &config).unwrap_err();
    assert!(err.contains("package name collision"));
    // Flatten joins the whole path into one segment, so the packages differ.
    let flat = CodegenConfig {
        flatten: true,
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    assert!(check_layout(&model, &[TargetKind::Go], &flat).is_ok());
}

/// A model with one entry declaring no fields, for the resolution-helper
/// pruning test below to add exactly the field it needs.
fn bare_entry_model() -> Model {
    Model {
        tono_ir_version: 6,
        modules: vec![Module {
            tests: vec![],
            name: "payments".into(),
            shapes: vec![Shape {
                id: "payments#client".into(),
                kind: ShapeKind::Entry {
                    fields: vec![],
                    operations: vec![],
                },
                traits: vec![],
            }],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    }
}

#[test]
fn rust_entry_resolution_helpers_prune_to_only_what_the_model_uses() {
    use crate::codegen::test_support::{bare_entry_field, push_entry_field};
    use crate::ir::{EntryField, EnvName, Source};

    // An entry with an env-sourced string field pulls in `read_env` alone:
    // no `@str::` transform and no duration field means the `casing` and
    // `duration` root groups never even appear, rather than shipping empty
    // or (as an earlier, unnamed-declaration approach did) shipping their
    // full contents unconditionally as dead code.
    let mut plain = bare_entry_model();
    push_entry_field(
        &mut plain.modules[0],
        bare_entry_field(
            "name",
            Tref::Prim(Prim::String),
            vec![Source::Env(EnvName::Name("NAME".into()))],
        ),
    );
    let files = generate(&plain, &[TargetKind::Rust], &CodegenConfig::default()).unwrap();
    let paths = paths_of(&files);
    assert!(paths.contains(&"rust/env.rs".to_string()));
    assert!(!paths.iter().any(|p| p.contains("casing")));
    assert!(!paths.iter().any(|p| p.contains("duration")));
    let env = text_at(&files, "rust/env.rs");
    assert!(env.contains("pub fn read_env"));
    let entry = text_at(&files, "rust/payments/client.rs");
    assert!(entry.contains("use crate::env::read_env;"));

    // A single `@str::kebab` transform pulls in exactly that transform (and
    // the word-splitter it shares the group with), not its three siblings.
    let mut kebabed = bare_entry_model();
    push_entry_field(
        &mut kebabed.modules[0],
        EntryField {
            transforms: vec!["kebab".to_string()],
            ..bare_entry_field(
                "name",
                Tref::Prim(Prim::String),
                vec![Source::Default(serde_json::json!("x"))],
            )
        },
    );
    let files = generate(&kebabed, &[TargetKind::Rust], &CodegenConfig::default()).unwrap();
    let paths = paths_of(&files);
    assert!(paths.contains(&"rust/casing.rs".to_string()));
    let casing = text_at(&files, "rust/casing.rs");
    assert!(casing.contains("fn str_kebab"));
    assert!(!casing.contains("fn str_snake"));
    assert!(!casing.contains("fn str_pascal"));
    assert!(!casing.contains("fn str_upper_snake"));
    assert!(!casing.contains("non_snake_case"));
}

#[test]
fn rust_entry_op_types_skip_a_redundant_import_for_same_module_types() {
    let model = Model {
        tono_ir_version: 6,
        modules: vec![
            Module {
                tests: vec![],
                name: "payments.common".into(),
                shapes: vec![structure(
                    "payments.common#money",
                    vec![member("amount", Tref::Prim(Prim::I64), true)],
                )],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
            Module {
                tests: vec![],
                name: "payments.charges".into(),
                shapes: vec![
                    structure(
                        "payments.charges#charge",
                        vec![member("id", Tref::Prim(Prim::String), true)],
                    ),
                    Shape {
                        id: "payments.charges#client".into(),
                        kind: ShapeKind::Entry {
                            fields: vec![EntryField {
                                name: "ep".into(),
                                target: Tref::Prim(Prim::String),
                                sources: vec![
                                    Source::With,
                                    Source::Default(serde_json::json!("https://example.com")),
                                ],
                                format: None,
                                transforms: vec![],
                                select: None,
                                call: None,
                                binds: vec![],
                                constraints: vec![],
                                traits: vec![],
                            }],
                            operations: vec![Shape {
                                id: "payments.charges#client.create".into(),
                                kind: ShapeKind::Operation {
                                    input_name: None,
                                    input: Some(Tref::Ref {
                                        id: "payments.charges#charge".into(),
                                        args: vec![],
                                    }),
                                    output: Some(Tref::Ref {
                                        id: "payments.common#money".into(),
                                        args: vec![],
                                    }),
                                    errors: vec![],
                                    wire: Some(Box::new(WireBinding {
                                        uri: WireValue::Template(vec![TemplatePart::Lit(
                                            "/charges".into(),
                                        )]),
                                        success: vec![200],
                                        endpoint: Some(WireValue::Field(vec!["ep".into()])),
                                        ..*wire_binding("POST")
                                    })),
                                    impl_call: None,
                                },
                                traits: vec![],
                            }],
                        },
                        traits: vec![],
                    },
                ],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            },
        ],
    };
    let files = generate(&model, &[TargetKind::Rust], &CodegenConfig::default()).unwrap();
    let entry = text_at(&files, "rust/payments/charges/client.rs");
    assert!(entry.contains("use crate::payments::charges::types::*;"));
    // The op's input type is declared in the entry's own module, already
    // covered by the glob above; a second, individually-collected import
    // naming it would be dead weight.
    assert!(!entry.contains("use crate::payments::charges::types::Charge;"));
    // The output type crosses modules, so it still needs its own import: the
    // glob only reaches the entry's own module.
    assert!(entry.contains("use crate::payments::common::types::Money;"));
}

/// A sweep of an output directory tells generated source from hand-written
/// source by this check, so it has to hold for real emitted files (whatever the
/// formatter did to the banner line) and stay false for anything else.
#[test]
fn is_generated_recognizes_emitted_source_only() {
    let model = union_model();
    for target in [TargetKind::Rust, TargetKind::Go, TargetKind::TypeScript] {
        for file in generate(&model, &[target], &CodegenConfig::default()).unwrap() {
            // The TypeScript package manifest is JSON, which carries no comment
            // and so is deliberately not identifiable; every source file is.
            if file.path.extension().and_then(|e| e.to_str()) == Some("json") {
                assert!(!is_generated(&file.text), "{}", file.path.display());
            } else {
                assert!(is_generated(&file.text), "{}", file.path.display());
            }
        }
    }
}

#[test]
fn is_generated_rejects_hand_written_source() {
    assert!(!is_generated("fn main() {}\n"));
    assert!(!is_generated(""));
    // The sentence buried in prose further down is not a banner: only the
    // opening of a file marks it as generated.
    let doc = "//! A module.\n//\n//\n//\n/// Code generated by tono. DO NOT EDIT.\nfn f() {}\n";
    assert!(!is_generated(doc));
}

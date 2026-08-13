//! The layout's tests, in a file of their own so the module stays inside the
//! source-size ceiling.

use super::*;
use crate::codegen::generate;
use crate::ir::{Module, Prim, Shape, ShapeKind, Tref};

fn path_of(target: TargetKind, grp: &Group) -> String {
    output_path(target, grp)
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn model(names: &[&str]) -> Model {
    Model {
        tono_ir_version: 6,
        modules: names
            .iter()
            .map(|name| Module {
                tests: vec![],
                name: (*name).into(),
                shapes: vec![],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            })
            .collect(),
    }
}

/// A model whose first module declares an entry, which is what puts anything
/// in the shared package.
fn model_with_entry(names: &[&str]) -> Model {
    let mut model = model(names);
    model.modules[0].shapes.push(crate::ir::Shape {
        id: format!("{}#client", names[0]),
        kind: crate::ir::ShapeKind::Entry {
            fields: vec![],
            operations: vec![],
        },
        traits: vec![],
    });
    model
}

#[test]
fn go_maps_a_module_to_a_package_directory_and_each_group_to_a_file() {
    assert_eq!(
        path_of(TargetKind::Go, &Group::types("payments.common")),
        "go/payments/common/types.go"
    );
    assert_eq!(
        path_of(TargetKind::Go, &Group::entry("payments.charges", "client")),
        "go/payments/charges/client.go"
    );
    // Each SDK-root group is a package of its own under internal/, named for
    // what it holds.
    assert_eq!(
        path_of(TargetKind::Go, &Group::root("duration")),
        "go/internal/duration/duration.go"
    );
    assert_eq!(
        path_of(TargetKind::Go, &Group::root("casing")),
        "go/internal/casing/casing.go"
    );
}

#[test]
fn a_test_group_takes_the_name_its_runner_discovers() {
    // `go test` compiles `_test.go` beside the package, Vitest picks up
    // `.test.ts`, and cargo compiles a `#[cfg(test)] mod` from `_test.rs`.
    let hermetic = Group::tests("payments.charges", "client", false);
    let live = Group::tests("payments.charges", "client", true);
    assert_eq!(
        path_of(TargetKind::Go, &hermetic),
        "go/payments/charges/client_test.go"
    );
    assert_eq!(
        path_of(TargetKind::Go, &live),
        "go/payments/charges/client_live_test.go"
    );
    assert_eq!(
        path_of(TargetKind::TypeScript, &hermetic),
        "typescript/payments/charges/client.test.ts"
    );
    assert_eq!(
        path_of(TargetKind::TypeScript, &live),
        "typescript/payments/charges/client.live.test.ts"
    );
    assert_eq!(
        path_of(TargetKind::Rust, &hermetic),
        "rust/payments/charges/client_test.rs"
    );
    assert_eq!(
        path_of(TargetKind::Rust, &live),
        "rust/payments/charges/client_live_test.rs"
    );
}

#[test]
fn rust_and_typescript_map_every_group_to_a_file() {
    assert_eq!(
        path_of(TargetKind::Rust, &Group::types("payments.common")),
        "rust/payments/common/types.rs"
    );
    // Rust fences with visibility, so nothing moves and no file is named for
    // its audience: the SDK-root group is a module named for its contents,
    // and a module's internal group rides its public file as `pub(crate)`.
    assert_eq!(
        path_of(TargetKind::Rust, &Group::root("duration")),
        "rust/duration.rs"
    );
    assert_eq!(
        path_of(
            TargetKind::Rust,
            &Group::module_internal("payments.charges")
        ),
        path_of(TargetKind::Rust, &Group::types("payments.charges"))
    );
    // The codec cannot move (an impl lives where its type does), so it is
    // fenced where it sits.
    assert_eq!(
        path_of(TargetKind::Rust, &Group::codec("payments.charges")),
        "rust/payments/charges/codec.rs"
    );
    assert_eq!(
        path_of(
            TargetKind::TypeScript,
            &Group::module_internal("payments.charges")
        ),
        path_of(TargetKind::TypeScript, &Group::types("payments.charges"))
    );
    assert_eq!(
        path_of(TargetKind::TypeScript, &Group::root("duration")),
        "typescript/duration.ts"
    );
    assert_eq!(
        path_of(TargetKind::TypeScript, &Group::root("casing")),
        "typescript/casing.ts"
    );
    assert_eq!(
        target_relative_path(TargetKind::TypeScript, &Group::types("notes"))
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
        "notes/types.ts"
    );
}

#[test]
fn go_groups_of_one_module_share_a_package() {
    assert!(same_go_package(
        "payments.charges::types",
        "payments.charges::codec"
    ));
    // The group Go moves under internal/ is a package of its own, which is
    // what puts it out of a consumer's reach.
    assert!(!same_go_package(
        "payments.charges::types",
        "payments.charges::internal"
    ));
    assert_eq!(
        path_of(TargetKind::Go, &Group::module_internal("payments.charges")),
        "go/internal/payments/charges/charges.go"
    );
    assert_eq!(
        go_import("example.com/sdk", "payments.charges::internal").as_deref(),
        Some("example.com/sdk/internal/payments/charges")
    );
    // The relocated package keeps the module's whole path, like the public
    // one: flattening to the last segment would land two modules that share
    // it on the same file.
    assert_ne!(
        path_of(TargetKind::Go, &Group::module_internal("payments.common")),
        path_of(TargetKind::Go, &Group::module_internal("billing.common"))
    );
    assert!(!same_go_package(
        "payments.charges::types",
        "payments.common::types"
    ));
    // The SDK-root group is its own package, and an external specifier is
    // never in anyone's package.
    assert!(!same_go_package("payments.charges::types", "::internal"));
    assert!(same_go_package("::internal", "::internal"));
    assert!(!same_go_package("payments.charges::types", "encoding/json"));
}

#[test]
fn import_paths_follow_each_language_idiom() {
    assert_eq!(
        rust_path("payments.common::types").as_deref(),
        Some("crate::payments::common::types")
    );
    assert_eq!(rust_path("::number").as_deref(), Some("crate::number"));
    assert_eq!(rust_path("::casing").as_deref(), Some("crate::casing"));
    assert_eq!(
        rust_path("payments.charges::internal").as_deref(),
        Some("crate::payments::charges::types")
    );

    assert_eq!(
        go_import("example.com/sdk", "payments.common::types").as_deref(),
        Some("example.com/sdk/payments/common")
    );
    assert_eq!(
        go_import("example.com/sdk", "::codec").as_deref(),
        Some("example.com/sdk/internal/codec")
    );
    assert_eq!(go_import("example.com/sdk", "encoding/json"), None);
    assert_eq!(
        go_selector("payments.common::types").as_deref(),
        Some("common")
    );
    assert_eq!(go_selector("::record").as_deref(), Some("record"));
    assert_eq!(go_selector("::casing").as_deref(), Some("casing"));

    assert_eq!(
        ts_specifier("payments.charges::types", "payments.common::types").as_deref(),
        Some("../common/types")
    );
    assert_eq!(
        ts_specifier("payments.charges::types", "::codec").as_deref(),
        Some("../../codec")
    );
    // A module's internal group shares its public file, so a reference to it
    // is a reference to that file.
    assert_eq!(
        ts_specifier("payments.charges::types", "payments.charges::internal").as_deref(),
        Some("./types")
    );
    assert_eq!(
        ts_specifier("payments.charges::types", "payments.charges::codec").as_deref(),
        Some("./codec")
    );
    assert_eq!(
        ts_specifier("notes::types", "::config").as_deref(),
        Some("../config")
    );
    assert_eq!(ts_specifier("notes::types", "@tono/http-runtime-ts"), None);
}

#[test]
fn the_shared_go_package_exists_only_when_something_uses_it() {
    // Its content is the entry construction helpers, so a model with no
    // entry declares nothing shared and stays a single package.
    assert!(!go_has_shared_package(&model(&["notes"])));
    assert!(go_has_shared_package(&model_with_entry(&["notes"])));
    // A second package means the cross-package imports need a module path.
    let err = check_layout(
        &model_with_entry(&["notes"]),
        &[TargetKind::Go],
        &CodegenConfig::default(),
    )
    .unwrap_err();
    assert!(err.contains("--go-module"));
}

/// A model whose first module has a wide-integer field, which is what puts
/// anything in the Rust crate's shared serialization module.
fn model_with_wide_int(names: &[&str]) -> Model {
    let mut model = model(names);
    model.modules[0]
        .shapes
        .push(crate::codegen::test_support::structure(
            &format!("{}#Charge", names[0]),
            vec![crate::codegen::test_support::member(
                "amount",
                Tref::Prim(Prim::I64),
                true,
            )],
        ));
    model
}

#[test]
fn a_module_taking_a_shared_modules_name_is_rejected() {
    // The shared group is a file at the root of the output and the module
    // would be a directory beside it: Rust reads both as one module and
    // refuses the crate, and a TypeScript relative import resolves to the
    // file. Either way the collision has to fail here, with a name to act on.
    let collides = model_with_wide_int(&["number", "notes"]);
    let err = check_layout(&collides, &[TargetKind::Rust], &CodegenConfig::default()).unwrap_err();
    assert!(err.contains("module name collision"), "got {err}");
    // Flatten joins the path into one segment, so a nested module clears it.
    let nested = model_with_wide_int(&["number.charges"]);
    assert!(
        check_layout(&nested, &[TargetKind::Rust], &CodegenConfig::default()).is_err(),
        "a first segment is enough to collide: it is the directory Rust sees"
    );
    let flat = CodegenConfig {
        flatten: true,
        ..CodegenConfig::default()
    };
    assert!(check_layout(&nested, &[TargetKind::Rust], &flat).is_ok());
    // With nothing shared there is no file to collide with.
    assert!(check_layout(
        &model(&["number"]),
        &[TargetKind::Rust],
        &CodegenConfig::default()
    )
    .is_ok());
    // TypeScript puts its shared groups at the package root too, so the
    // same module collides there.
    assert!(check_layout(
        &model_with_wide_int(&["number"]),
        &[TargetKind::TypeScript],
        &CodegenConfig::default()
    )
    .is_err());
    // Go keeps its shared packages under `internal/`, where no module reaches.
    let go = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    assert!(check_layout(&model_with_wide_int(&["number"]), &[TargetKind::Go], &go).is_ok());
}

#[test]
fn go_multi_module_without_a_module_path_is_rejected() {
    let model = model(&["payments.common", "payments.charges"]);
    let err = check_layout(&model, &[TargetKind::Go], &CodegenConfig::default()).unwrap_err();
    assert!(err.contains("--go-module"));
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    assert!(check_layout(&model, &[TargetKind::Go], &config).is_ok());
    // Rust and TypeScript have relative imports, so the same layout is fine.
    assert!(check_layout(
        &model,
        &[TargetKind::Rust, TargetKind::TypeScript],
        &CodegenConfig::default()
    )
    .is_ok());
}

#[test]
fn go_modules_sharing_a_last_segment_are_rejected() {
    let model = model(&["a.common", "b.common"]);
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

#[test]
fn the_shared_package_name_is_reserved_from_the_modules() {
    let config = CodegenConfig {
        go_module: Some("example.com/sdk".into()),
        ..CodegenConfig::default()
    };
    let err = check_layout(
        &model_with_entry(&["tono", "notes"]),
        &[TargetKind::Go],
        &config,
    )
    .unwrap_err();
    assert!(err.contains("shared internal package"));
    // With nothing shared there is no such package, so the name is free.
    assert!(check_layout(&model(&["tono", "notes"]), &[TargetKind::Go], &config).is_ok());
}

/// A module declaring two entries, one of them named something other than
/// `client`, so the layout has to name a group after each declaration.
fn two_entry_model() -> Model {
    let field = crate::ir::EntryField {
        name: "endpoint".into(),
        target: Tref::Prim(Prim::String),
        sources: vec![],
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![crate::ir::Trait {
            id: "arg".into(),
            value: serde_json::Value::Null,
        }],
    };
    let entry = |name: &str| Shape {
        id: format!("notes#{name}"),
        kind: ShapeKind::Entry {
            fields: vec![field.clone()],
            operations: vec![],
        },
        traits: vec![],
    };
    Model {
        tono_ir_version: 6,
        modules: vec![Module {
            tests: vec![],
            name: "notes".into(),
            shapes: vec![entry("admin"), entry("reader")],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
        }],
    }
}

#[test]
fn each_entry_declaration_gets_a_group_named_after_it() {
    for (target, ext) in [(TargetKind::Go, "go"), (TargetKind::TypeScript, "ts")] {
        let config = CodegenConfig {
            go_module: Some("example.com/sdk".into()),
            ..CodegenConfig::default()
        };
        let files = generate(&two_entry_model(), &[target], &config).unwrap();
        let paths = files
            .iter()
            .map(|f| {
                f.path
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
            })
            .collect::<Vec<_>>();
        let dir = target.dir();
        // Two entries in one module are two groups, each named for its
        // declaration rather than for a fixed `client`.
        assert!(paths.contains(&format!("{dir}/notes/admin.{ext}")));
        assert!(paths.contains(&format!("{dir}/notes/reader.{ext}")));
        // Each holds its own constructor, not the other's.
        let admin = &files
            .iter()
            .find(|f| {
                f.path
                    .to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/")
                    == format!("{dir}/notes/admin.{ext}")
            })
            .expect("the entry's own group")
            .text;
        assert!(admin.contains("Admin"));
        assert!(!admin.contains("Reader"));
    }
}

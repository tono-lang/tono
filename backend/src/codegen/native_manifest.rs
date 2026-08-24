//! What the emitted source requires from the consuming package's native
//! manifest, stated once so the CLI (scaffolding and merging `Cargo.toml`)
//! and the compile-check scaffold cannot drift from what the emitters
//! actually reference.
//!
//! Only Rust carries requirements beyond the standard library: the types
//! derive serde, and an entry client's inline transport calls reqwest behind
//! the crate's default-on `reqwest` feature with tokio for its timing. Go's
//! emitted transport is `net/http`, and TypeScript's is `fetch`, so their
//! manifests need no dependency lines at all.

use crate::codegen::entries;
use crate::ir::Model;

/// One `[dependencies]` line: the crate name and the raw TOML value of its
/// requirement, written as-is into the manifest.
pub type CargoDep = (&'static str, &'static str);

/// The serde derive the generated types carry.
pub const SERDE: CargoDep = ("serde", "{ version = \"1\", features = [\"derive\"] }");

/// The JSON codec the generated serde helpers and transport bodies use.
pub const SERDE_JSON: CargoDep = ("serde_json", "\"1\"");

/// The native HTTP stack of the emitted transport. Optional: the `reqwest`
/// feature is default-on, and a consumer that supplies the canonical
/// transport slot instead builds with `--no-default-features`.
pub const REQWEST: CargoDep = (
    "reqwest",
    "{ version = \"0.12\", default-features = false, features = [\"rustls-tls\"], optional = true }",
);

/// The async runtime the emitted transport's retry/timeout paths and the
/// generated tests run on.
pub const TOKIO: CargoDep = (
    "tokio",
    "{ version = \"1\", features = [\"rt-multi-thread\", \"macros\", \"net\", \"io-util\", \"time\"] }",
);

/// The `[features]` entries wiring the optional native transport: present
/// exactly when [`rust_dependencies`] includes reqwest.
pub const RUST_TRANSPORT_FEATURES: &[(&str, &str)] = &[
    ("default", "[\"reqwest\"]"),
    ("reqwest", "[\"dep:reqwest\"]"),
];

/// Whether the model emits an entry client (and with it the inline HTTP
/// transport), which is what pulls the transport dependencies in.
fn emits_transport(model: &Model) -> bool {
    model.modules.iter().any(entries::has_entries)
}

/// The `[dependencies]` lines the Rust emitted for `model` requires. The ext
/// libraries the model binds are not listed here: their versions are the
/// project manifest's to pin, not the generator's to invent.
pub fn rust_dependencies(model: &Model) -> Vec<CargoDep> {
    let mut deps = vec![SERDE, SERDE_JSON];
    if emits_transport(model) {
        deps.push(REQWEST);
        deps.push(TOKIO);
    }
    deps
}

/// The `[features]` entries the Rust emitted for `model` requires: the
/// transport feature wiring when an entry client is emitted, nothing
/// otherwise.
pub fn rust_features(model: &Model) -> &'static [(&'static str, &'static str)] {
    if emits_transport(model) {
        RUST_TRANSPORT_FEATURES
    } else {
        &[]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::structure;
    use crate::ir::{Module, Shape, ShapeKind};

    fn model_with(shapes: Vec<Shape>) -> Model {
        Model {
            tono_ir_version: crate::ir::TONO_IR_VERSION,
            modules: vec![Module {
                name: "demo".into(),
                shapes,
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
                tests: vec![],
            }],
        }
    }

    fn entry_shape() -> Shape {
        Shape {
            id: "demo#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![],
                operations: vec![],
            },
            traits: vec![],
        }
    }

    fn struct_shape() -> Shape {
        structure("demo#charge", vec![])
    }

    #[test]
    fn a_types_only_model_needs_only_serde() {
        let model = model_with(vec![struct_shape()]);
        let names: Vec<&str> = rust_dependencies(&model).iter().map(|d| d.0).collect();
        assert_eq!(names, ["serde", "serde_json"]);
        assert!(rust_features(&model).is_empty());
    }

    #[test]
    fn a_model_with_an_entry_pulls_the_transport_stack() {
        let model = model_with(vec![struct_shape(), entry_shape()]);
        let names: Vec<&str> = rust_dependencies(&model).iter().map(|d| d.0).collect();
        assert_eq!(names, ["serde", "serde_json", "reqwest", "tokio"]);
        let features: Vec<&str> = rust_features(&model).iter().map(|f| f.0).collect();
        assert_eq!(features, ["default", "reqwest"]);
    }
}

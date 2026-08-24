//! Mapping the canonical dotted module name to each target's idiomatic
//! sub-package, plus the config hooks that steer it.
//!
//! A `.tono` file's path is its module name (`payments/charge.tono` ->
//! `payments.charge`), which the IR carries dotted in every `module#name` id. The
//! default mapping turns that hierarchy into an idiomatic sub-package per target
//! (Rust `payments::charge`, Go/TypeScript `payments/charge`); the two hooks here
//! rewrite the canonical name first: `remap` rewrites a matching prefix and
//! `flatten` collapses the whole hierarchy into a single segment.
//!
//! Config is applied once, up front, by rewriting the module part of every id in
//! the model ([`apply`]); the render rules and output paths then see only the
//! effective names and never need to know about the config.

use crate::ir::{EntryField, Extension, Member, Model, Module, Shape, ShapeKind, Signature, Tref};

/// Config hooks consumed by codegen when mapping modules to packages. The default
/// (no flatten, no remap) is the idiomatic sub-package mapping.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CodegenConfig {
    /// Collapse the dotted hierarchy into one flat segment (dots become
    /// underscores), so no sub-packages are produced.
    pub flatten: bool,
    /// Rewrite a matching dotted module prefix as `(from, to)`; the first match
    /// wins and a prefix only matches on a segment boundary.
    pub remap: Vec<(String, String)>,
    /// The generated Go SDK's module path (from `--go-module`). Go has no relative
    /// imports, so a cross-package import needs this as a prefix
    /// (`<go_module>/payments/common`). `None` suits the flat single-package
    /// layout, where no cross-package import arises.
    pub go_module: Option<String>,
}

/// Map a canonical module name through the config: apply the first matching remap
/// prefix, then flatten if requested.
pub fn canonicalize(config: &CodegenConfig, module: &str) -> String {
    let remapped = config
        .remap
        .iter()
        .find_map(|(from, to)| remap_prefix(from, to, module))
        .unwrap_or_else(|| module.to_string());
    if config.flatten {
        remapped.replace('.', "_")
    } else {
        remapped
    }
}

/// Rewrite `module` when `from` matches it exactly or as a leading dotted-segment
/// prefix, so `payments` matches `payments` and `payments.common` but not
/// `payments_v2`.
fn remap_prefix(from: &str, to: &str, module: &str) -> Option<String> {
    if module == from {
        Some(to.to_string())
    } else {
        module
            .strip_prefix(from)
            .filter(|rest| rest.starts_with('.'))
            .map(|rest| format!("{to}{rest}"))
    }
}

/// Rewrite every module name in the model, the module names themselves and the
/// module part of each `module#name` id, through the config. Local names, core
/// trait ids, and everything else are untouched.
pub fn apply(config: &CodegenConfig, model: &Model) -> Model {
    Model {
        tono_ir_version: model.tono_ir_version,
        modules: model.modules.iter().map(|m| module(config, m)).collect(),
    }
}

fn module(config: &CodegenConfig, m: &Module) -> Module {
    Module {
        name: canonicalize(config, &m.name),
        shapes: m.shapes.iter().map(|s| shape(config, s)).collect(),
        operations: m.operations.iter().map(|s| shape(config, s)).collect(),
        extensions: m.extensions.iter().map(|e| extension(config, e)).collect(),
        ext_libs: m.ext_libs.clone(),
        tests: m.tests.clone(),
    }
}

/// Remap the shape ids a contract's signature references; bindings are
/// file#symbol paths and conformance is an external reference, both left alone.
fn extension(config: &CodegenConfig, e: &Extension) -> Extension {
    Extension {
        signature: e.signature.as_ref().map(|s| Signature {
            input: tref(config, &s.input),
            output: tref(config, &s.output),
        }),
        ..e.clone()
    }
}

fn shape(config: &CodegenConfig, s: &Shape) -> Shape {
    Shape {
        id: id(config, &s.id),
        kind: kind(config, &s.kind),
        traits: s.traits.clone(),
    }
}

/// Rewrite the module part of a `module#name` id; a bare id (no `#`) is left
/// alone.
fn id(config: &CodegenConfig, raw: &str) -> String {
    match raw.split_once('#') {
        Some((module, local)) => format!("{}#{}", canonicalize(config, module), local),
        None => raw.to_string(),
    }
}

fn kind(config: &CodegenConfig, k: &ShapeKind) -> ShapeKind {
    match k {
        ShapeKind::Structure { params, members } => ShapeKind::Structure {
            params: params.clone(),
            members: members.iter().map(|m| member(config, m)).collect(),
        },
        ShapeKind::Union {
            params,
            members,
            discriminator,
        } => ShapeKind::Union {
            params: params.clone(),
            members: members.iter().map(|m| member(config, m)).collect(),
            discriminator: discriminator.clone(),
        },
        ShapeKind::Enum { .. } => k.clone(),
        ShapeKind::Service { operations } => ShapeKind::Service {
            operations: operations.iter().map(|op| id(config, op)).collect(),
        },
        ShapeKind::Operation {
            input,
            input_name,
            output,
            output_nullable,
            errors,
            wire,
            impl_call,
        } => ShapeKind::Operation {
            input: input.as_ref().map(|t| tref(config, t)),
            input_name: input_name.clone(),
            output: output.as_ref().map(|t| tref(config, t)),
            output_nullable: *output_nullable,
            errors: errors.iter().map(|t| tref(config, t)).collect(),
            // Nothing inside a wire binding carries a shape id to rewrite
            // (its refs are entry-field paths and literal HTTP names).
            wire: wire.clone(),
            // Nothing inside an impl_call carries a shape id either (its
            // refs are entry-field paths and a bare method name).
            impl_call: impl_call.clone(),
        },
        ShapeKind::Entry { fields, operations } => ShapeKind::Entry {
            fields: fields.iter().map(|f| entry_field(config, f)).collect(),
            operations: operations.iter().map(|op| shape(config, op)).collect(),
        },
        ShapeKind::Config { fields } => ShapeKind::Config {
            fields: fields.iter().map(|f| entry_field(config, f)).collect(),
        },
    }
}

// Entry-field sources, binds, and templates carry field names, never module
// ids; only the target type crosses module namespaces.
fn entry_field(config: &CodegenConfig, f: &EntryField) -> EntryField {
    EntryField {
        target: tref(config, &f.target),
        ..f.clone()
    }
}

fn member(config: &CodegenConfig, m: &Member) -> Member {
    Member {
        target: tref(config, &m.target),
        ..m.clone()
    }
}

fn tref(config: &CodegenConfig, t: &Tref) -> Tref {
    match t {
        Tref::Ref { id: raw, args } => Tref::Ref {
            id: id(config, raw),
            args: args.iter().map(|a| tref(config, a)).collect(),
        },
        Tref::List(inner) => Tref::List(Box::new(tref(config, inner))),
        Tref::Map(k, v) => Tref::Map(Box::new(tref(config, k)), Box::new(tref(config, v))),
        Tref::Prim(_) | Tref::Param(_) => t.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonicalize_default_is_identity() {
        let c = CodegenConfig::default();
        assert_eq!(canonicalize(&c, "payments.common"), "payments.common");
        assert_eq!(canonicalize(&c, "payments"), "payments");
    }

    #[test]
    fn flatten_collapses_dots_to_underscores() {
        let c = CodegenConfig {
            flatten: true,
            remap: vec![],
            go_module: None,
        };
        assert_eq!(canonicalize(&c, "payments.common"), "payments_common");
        assert_eq!(canonicalize(&c, "payments"), "payments");
    }

    #[test]
    fn remap_rewrites_a_prefix_on_segment_boundaries() {
        let c = CodegenConfig {
            flatten: false,
            remap: vec![("payments".into(), "billing".into())],
            go_module: None,
        };
        assert_eq!(canonicalize(&c, "payments"), "billing");
        assert_eq!(canonicalize(&c, "payments.common"), "billing.common");
        // Not a segment boundary: left untouched.
        assert_eq!(canonicalize(&c, "payments_v2"), "payments_v2");
    }

    #[test]
    fn remap_then_flatten_compose() {
        let c = CodegenConfig {
            flatten: true,
            remap: vec![("payments".into(), "billing.core".into())],
            go_module: None,
        };
        assert_eq!(canonicalize(&c, "payments.common"), "billing_core_common");
    }

    #[test]
    fn apply_rewrites_ids_but_not_trait_ids() {
        use crate::ir::Trait;
        let model = Model {
            tono_ir_version: 2,
            modules: vec![Module {
                tests: vec![],
                name: "payments.common".into(),
                shapes: vec![Shape {
                    id: "payments.common#money".into(),
                    kind: ShapeKind::Structure {
                        params: vec![],
                        members: vec![Member {
                            name: "parent".into(),
                            target: Tref::Ref {
                                id: "payments.common#money".into(),
                                args: vec![],
                            },
                            required: true,
                            default: None,
                            constraints: vec![],
                            traits: vec![],
                        }],
                    },
                    traits: vec![Trait {
                        id: "doc".into(),
                        value: serde_json::Value::String("x".into()),
                    }],
                }],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            }],
        };
        let c = CodegenConfig {
            flatten: false,
            remap: vec![("payments".into(), "billing".into())],
            go_module: None,
        };
        let out = apply(&c, &model);
        let m = &out.modules[0];
        assert_eq!(m.name, "billing.common");
        assert_eq!(m.shapes[0].id, "billing.common#money");
        // The trait id (core namespace) is left alone.
        assert_eq!(m.shapes[0].traits[0].id, "doc");
        match &m.shapes[0].kind {
            ShapeKind::Structure { members, .. } => match &members[0].target {
                Tref::Ref { id, .. } => assert_eq!(id, "billing.common#money"),
                _ => panic!("expected a ref"),
            },
            _ => panic!("expected a structure"),
        }
    }

    #[test]
    fn apply_rewrites_entry_and_config_targets() {
        use crate::ir::EntryField;
        let field = |name: &str, target_id: &str| EntryField {
            name: name.into(),
            target: Tref::Ref {
                id: target_id.into(),
                args: vec![],
            },
            sources: vec![],
            format: None,
            transforms: vec![],
            select: None,
            call: None,
            handle_call: None,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        };
        let model = Model {
            tono_ir_version: 5,
            modules: vec![Module {
                tests: vec![],
                name: "payments.api".into(),
                shapes: vec![
                    Shape {
                        id: "payments.api#client".into(),
                        kind: ShapeKind::Entry {
                            fields: vec![field("settings", "payments.api#conf")],
                            operations: vec![Shape {
                                id: "payments.api#client.save".into(),
                                kind: ShapeKind::Operation {
                                    input: Some(Tref::Ref {
                                        id: "payments.api#note".into(),
                                        args: vec![],
                                    }),
                                    input_name: None,
                                    output: None,
                                    output_nullable: false,
                                    errors: vec![],
                                    wire: None,
                                    impl_call: None,
                                },
                                traits: vec![],
                            }],
                        },
                        traits: vec![],
                    },
                    Shape {
                        id: "payments.api#conf".into(),
                        kind: ShapeKind::Config {
                            fields: vec![field("peer", "payments.api#note")],
                        },
                        traits: vec![],
                    },
                ],
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            }],
        };
        let c = CodegenConfig {
            flatten: false,
            remap: vec![("payments".into(), "billing".into())],
            go_module: None,
        };
        let out = apply(&c, &model);
        let target_id = |f: &EntryField| match &f.target {
            Tref::Ref { id, .. } => id.clone(),
            _ => panic!("expected a ref"),
        };
        match &out.modules[0].shapes[0].kind {
            ShapeKind::Entry { fields, operations } => {
                assert_eq!(target_id(&fields[0]), "billing.api#conf");
                assert_eq!(operations[0].id, "billing.api#client.save");
                match &operations[0].kind {
                    ShapeKind::Operation {
                        input: Some(Tref::Ref { id, .. }),
                        ..
                    } => {
                        assert_eq!(id, "billing.api#note");
                    }
                    _ => panic!("expected an operation with a ref input"),
                }
            }
            _ => panic!("expected an entry"),
        }
        match &out.modules[0].shapes[1].kind {
            ShapeKind::Config { fields } => {
                assert_eq!(target_id(&fields[0]), "billing.api#note");
            }
            _ => panic!("expected a config"),
        }
    }
}

//! Which of a module's declarations belong to its public group.
//!
//! Visibility is derived, never written per declaration in the emitters. A
//! group supplies the default, and a declaration overrides it only through what
//! the spec already says: the `pub` marker a declaration carries. What the marker
//! cannot say, structure does. A type reachable from a public type is itself
//! public, because a consumer that can hold the outer value can already name the
//! inner one; hiding it would only make the public type unusable (in Rust,
//! literally uncompilable: a private type in a public interface).
//!
//! So `card`, unmarked in the spec but folded into a public union, comes out
//! exported; a helper shape no public type reaches stays in the module's
//! internal group.

use std::collections::{BTreeSet, HashMap};

use crate::ir::{Model, Module, Shape, ShapeKind, Tref};

/// The `pub` marker the frontend lowers a public declaration to.
const PUB_TRAIT: &str = "pub";

/// Whether a declaration is marked public in the spec.
fn is_marked_public(shape: &Shape) -> bool {
    shape.traits.iter().any(|t| t.id == PUB_TRAIT)
}

/// The shape ids a module exposes: the ones marked public in the spec, plus
/// everything structurally reachable from them.
#[derive(Debug, Default, Clone)]
pub struct Exposed {
    ids: BTreeSet<String>,
    all: bool,
}

impl Exposed {
    /// Everything exposed. For a caller emitting one module on its own, with no
    /// model to derive reachability over.
    pub fn all() -> Self {
        Self {
            ids: BTreeSet::new(),
            all: true,
        }
    }

    pub fn contains(&self, id: &str) -> bool {
        self.all || self.ids.contains(id)
    }

    /// Whether a shape belongs to its module's public group.
    pub fn shape(&self, shape: &Shape) -> bool {
        self.contains(&shape.id)
    }
}

/// Derive the exposed set over a whole model, since reachability crosses module
/// boundaries (a public type in one module can hold another module's shape).
pub fn derive(model: &Model) -> Exposed {
    // A spec that never marks anything public is not asking for anything to be
    // hidden: visibility is opt-in, so with no marker anywhere the whole model
    // is the SDK's surface.
    if !model
        .modules
        .iter()
        .flat_map(|m| m.shapes.iter())
        .any(is_marked_public)
    {
        return Exposed::all();
    }
    let by_id: HashMap<&str, &Shape> = model
        .modules
        .iter()
        .flat_map(|m| m.shapes.iter().chain(m.operations.iter()))
        .map(|s| (s.id.as_str(), s))
        .collect();

    let mut exposed = BTreeSet::new();
    let mut queue: Vec<String> = Vec::new();
    for module in &model.modules {
        for seed in seeds(module) {
            queue.push(seed);
        }
        // A name another module references is part of the surface whether or not
        // it is marked: a target that hides it (Go moves an internal group into
        // a package the other module cannot import) would emit source that does
        // not build.
        for shape in module.shapes.iter().chain(module.operations.iter()) {
            for id in references(shape) {
                if id.split_once('#').is_some_and(|(m, _)| m != module.name) {
                    queue.push(id);
                }
            }
        }
    }
    while let Some(id) = queue.pop() {
        if !exposed.insert(id.clone()) {
            continue;
        }
        let Some(shape) = by_id.get(id.as_str()) else {
            continue;
        };
        for reference in references(shape) {
            queue.push(reference);
        }
    }
    Exposed {
        ids: exposed,
        all: false,
    }
}

/// A module's public roots: every declaration the spec marks `pub`, every entry
/// (its whole point is to be constructed by a consumer), and every loose
/// operation.
///
/// A loose operation is a root whether or not it is marked, because the ones a
/// module declares become a single client interface, and that interface is
/// public: hiding one operation's types would leave a public signature naming
/// them. Honoring the marker on an operation means splitting that interface by
/// visibility first, which is a change to the client surface rather than to the
/// layout.
fn seeds(module: &Module) -> Vec<String> {
    module
        .shapes
        .iter()
        .filter(|s| is_marked_public(s) || matches!(s.kind, ShapeKind::Entry { .. }))
        .chain(module.operations.iter())
        .map(|s| s.id.clone())
        .chain(
            // A bespoke boundary's signature is part of the surface a hook or a
            // contract is written against, so its types are exposed too.
            module.extensions.iter().flat_map(|e| {
                e.signature
                    .iter()
                    .flat_map(|s| refs_in(&s.input).into_iter().chain(refs_in(&s.output)))
            }),
        )
        .collect()
}

/// The shape ids a shape reaches directly.
fn references(shape: &Shape) -> Vec<String> {
    match &shape.kind {
        ShapeKind::Structure { members, .. } | ShapeKind::Union { members, .. } => {
            members.iter().flat_map(|m| refs_in(&m.target)).collect()
        }
        // wire carries entry-field paths and literal HTTP names, never a
        // shape id, so it contributes no references here.
        ShapeKind::Operation {
            input,
            output,
            errors,
            ..
        } => input
            .iter()
            .chain(output.iter())
            .chain(errors.iter())
            .flat_map(refs_in)
            .collect(),
        ShapeKind::Entry { fields, operations } => fields
            .iter()
            .flat_map(|f| refs_in(&f.target))
            .chain(operations.iter().flat_map(references))
            .collect(),
        ShapeKind::Config { fields } => fields.iter().flat_map(|f| refs_in(&f.target)).collect(),
        ShapeKind::Enum { .. } | ShapeKind::Service { .. } => Vec::new(),
    }
}

/// Every shape id a type reference mentions, including the arguments of a
/// generic application and the element types of a collection.
fn refs_in(t: &Tref) -> Vec<String> {
    match t {
        Tref::Ref { id, args } => std::iter::once(id.clone())
            .chain(args.iter().flat_map(refs_in))
            .collect(),
        Tref::List(inner) => refs_in(inner),
        Tref::Map(key, value) => refs_in(key).into_iter().chain(refs_in(value)).collect(),
        Tref::Prim(_) | Tref::Param(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::test_support::{member, structure, union_shape};
    use crate::ir::{EntryField, Prim, Trait};

    fn public(mut shape: Shape) -> Shape {
        shape.traits.push(Trait {
            id: PUB_TRAIT.into(),
            value: serde_json::Value::Null,
        });
        shape
    }

    fn refers(name: &str, to: &str) -> Shape {
        structure(
            name,
            vec![member(
                "field",
                Tref::Ref {
                    id: to.into(),
                    args: vec![],
                },
                true,
            )],
        )
    }

    fn model(shapes: Vec<Shape>) -> Model {
        Model {
            tono_ir_version: 6,
            modules: vec![Module {
                tests: vec![],
                name: "m".into(),
                shapes,
                operations: vec![],
                extensions: vec![],
                ext_libs: vec![],
            }],
        }
    }

    #[test]
    fn an_unmarked_shape_reachable_from_a_public_union_is_exposed() {
        // The payments case: `card` is private in the spec but folded into a
        // public union, so hiding it would make the union unusable.
        let exposed = derive(&model(vec![
            public(union_shape(
                "m#payment_method",
                "kind",
                vec![member(
                    "card",
                    Tref::Ref {
                        id: "m#card".into(),
                        args: vec![],
                    },
                    true,
                )],
            )),
            structure(
                "m#card",
                vec![member("last4", Tref::Prim(Prim::String), true)],
            ),
        ]));
        assert!(exposed.contains("m#payment_method"));
        assert!(exposed.contains("m#card"));
    }

    #[test]
    fn a_model_that_marks_nothing_public_exposes_everything() {
        // Visibility is opt-in: with no marker anywhere, the spec is not asking
        // for anything to be hidden.
        let exposed = derive(&model(vec![
            structure("m#a", vec![member("v", Tref::Prim(Prim::String), true)]),
            structure("m#b", vec![member("v", Tref::Prim(Prim::String), true)]),
        ]));
        assert!(exposed.contains("m#a"));
        assert!(exposed.contains("m#b"));
    }

    #[test]
    fn a_pub_marked_shape_nothing_references_is_still_exposed() {
        // Reachability here is about what a *consumer* can reach, not what the
        // spec's own declarations reach: a `pub` marker is the contract, so it
        // is a root regardless of whether anything in the module happens to
        // reference it. Emitting only what is reachable must never silently
        // drop a declaration the spec itself asked to be public.
        let exposed = derive(&model(vec![
            public(structure(
                "m#unused_but_public",
                vec![member("v", Tref::Prim(Prim::String), true)],
            )),
            public(structure(
                "m#other",
                vec![member("v", Tref::Prim(Prim::String), true)],
            )),
        ]));
        assert!(exposed.contains("m#unused_but_public"));
    }

    #[test]
    fn an_unmarked_shape_nothing_public_reaches_stays_internal() {
        let exposed = derive(&model(vec![
            public(structure(
                "m#charge",
                vec![member("amount", Tref::Prim(Prim::I64), true)],
            )),
            structure("m#page", vec![member("size", Tref::Prim(Prim::I32), true)]),
        ]));
        assert!(exposed.contains("m#charge"));
        assert!(!exposed.contains("m#page"));
    }

    #[test]
    fn reachability_is_transitive_and_crosses_modules() {
        let model = Model {
            tono_ir_version: 6,
            modules: vec![
                Module {
                    tests: vec![],
                    name: "a".into(),
                    shapes: vec![public(refers("a#outer", "b#middle"))],
                    operations: vec![],
                    extensions: vec![],
                    ext_libs: vec![],
                },
                Module {
                    tests: vec![],
                    name: "b".into(),
                    shapes: vec![
                        refers("b#middle", "b#inner"),
                        structure("b#inner", vec![member("v", Tref::Prim(Prim::String), true)]),
                        structure("b#loose", vec![member("v", Tref::Prim(Prim::String), true)]),
                    ],
                    operations: vec![],
                    extensions: vec![],
                    ext_libs: vec![],
                },
            ],
        };
        let exposed = derive(&model);
        assert!(exposed.contains("b#middle"));
        assert!(exposed.contains("b#inner"));
        assert!(!exposed.contains("b#loose"));
    }

    #[test]
    fn an_entry_and_everything_its_operations_touch_are_exposed() {
        let op = Shape {
            id: "m#client.save".into(),
            kind: ShapeKind::Operation {
                input: Some(Tref::Ref {
                    id: "m#note".into(),
                    args: vec![],
                }),
                input_name: None,
                output: None,
                errors: vec![Tref::Ref {
                    id: "m#not_found".into(),
                    args: vec![],
                }],
                wire: None,
            },
            traits: vec![],
        };
        let entry = Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields: vec![EntryField {
                    name: "endpoint".into(),
                    target: Tref::Prim(Prim::String),
                    sources: vec![],
                    format: None,
                    transforms: vec![],
                    select: None,
                    call: None,
                    binds: vec![],
                    constraints: vec![],
                    traits: vec![],
                }],
                operations: vec![op],
            },
            traits: vec![],
        };
        let exposed = derive(&model(vec![
            entry,
            // A marked shape elsewhere is what turns visibility on at all.
            public(structure(
                "m#marked",
                vec![member("v", Tref::Prim(Prim::String), true)],
            )),
            structure(
                "m#note",
                vec![member("body", Tref::Prim(Prim::String), true)],
            ),
            structure(
                "m#not_found",
                vec![member("message", Tref::Prim(Prim::String), true)],
            ),
            structure(
                "m#helper",
                vec![member("v", Tref::Prim(Prim::String), true)],
            ),
        ]));
        // The entry is a construction surface, so it needs no `pub` marker, and
        // its operations carry their input and error types with them.
        assert!(exposed.contains("m#client"));
        assert!(exposed.contains("m#note"));
        assert!(exposed.contains("m#not_found"));
        assert!(!exposed.contains("m#helper"));
    }

    #[test]
    fn a_shape_another_module_references_is_exposed() {
        // Hiding it would put it where the referencing module cannot reach it.
        let model = Model {
            tono_ir_version: 6,
            modules: vec![
                Module {
                    tests: vec![],
                    name: "a".into(),
                    shapes: vec![refers("a#local", "b#shared")],
                    operations: vec![],
                    extensions: vec![],
                    ext_libs: vec![],
                },
                Module {
                    tests: vec![],
                    name: "b".into(),
                    shapes: vec![
                        public(structure(
                            "b#marked",
                            vec![member("v", Tref::Prim(Prim::String), true)],
                        )),
                        structure(
                            "b#shared",
                            vec![member("v", Tref::Prim(Prim::String), true)],
                        ),
                        structure("b#own", vec![member("v", Tref::Prim(Prim::String), true)]),
                    ],
                    operations: vec![],
                    extensions: vec![],
                    ext_libs: vec![],
                },
            ],
        };
        let exposed = derive(&model);
        assert!(exposed.contains("b#shared"));
        assert!(!exposed.contains("b#own"));
    }

    #[test]
    fn a_cycle_terminates() {
        let exposed = derive(&model(vec![
            public(refers("m#a", "m#b")),
            refers("m#b", "m#a"),
        ]));
        assert!(exposed.contains("m#a"));
        assert!(exposed.contains("m#b"));
    }

    #[test]
    fn generic_arguments_and_collection_elements_are_reached() {
        let exposed = derive(&model(vec![
            public(structure(
                "m#holder",
                vec![
                    member(
                        "pages",
                        Tref::List(Box::new(Tref::Ref {
                            id: "m#page".into(),
                            args: vec![Tref::Ref {
                                id: "m#item".into(),
                                args: vec![],
                            }],
                        })),
                        true,
                    ),
                    member(
                        "by_key",
                        Tref::Map(
                            Box::new(Tref::Prim(Prim::String)),
                            Box::new(Tref::Ref {
                                id: "m#value".into(),
                                args: vec![],
                            }),
                        ),
                        true,
                    ),
                ],
            )),
            structure("m#page", vec![]),
            structure("m#item", vec![]),
            structure("m#value", vec![]),
        ]));
        for id in ["m#page", "m#item", "m#value"] {
            assert!(exposed.contains(id), "{id} should be exposed");
        }
    }
}

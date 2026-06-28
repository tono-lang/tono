//! Deterministic, transitive import collection.
//!
//! A file's import set is never written by hand: it is folded from the symbols
//! reachable from the file's declarations. Walking gathers each referenced
//! symbol's import, recurses into the symbol's `references` (so a generic
//! `Page<Charge>` pulls both `Page` and `Charge`), deduplicates, and orders
//! deterministically so the output is byte-stable across runs. Imports whose
//! module equals the file's own module are dropped, since a type defined in this
//! file needs no import.

use std::collections::{BTreeSet, HashSet};

use crate::codegen::symbol::{Import, Symbol};
use crate::codegen::tree::{Decl, Field, File, FnBody, TypeExpr};

/// Collect the deduplicated, deterministically-ordered import set of a file.
///
/// Determinism comes from a `BTreeSet`, which orders imports by `(module,
/// imported)` regardless of discovery order; the self-module imports are
/// filtered out on the way to the returned `Vec`.
pub fn collect(file: &File) -> Vec<Import> {
    collect_with_companion(file, None)
}

/// Like [`collect`], but for a split-out file whose companion holds the module's
/// types. A self-module symbol is then not dropped but re-pointed at `companion`
/// (a module path), so the serde file imports each type it references from the
/// types file. With `companion` `None` this is exactly [`collect`].
pub fn collect_with_companion(file: &File, companion: Option<&str>) -> Vec<Import> {
    let mut acc: BTreeSet<Import> = BTreeSet::new();
    // Guards against reference cycles and skips re-walking a symbol already
    // seen. Keyed on (name, import) so two distinct same-named symbols from
    // different modules are still both visited.
    let mut visited: HashSet<(String, Option<Import>)> = HashSet::new();
    for decl in &file.decls {
        walk_decl(decl, &mut acc, &mut visited);
    }
    acc.into_iter()
        .filter_map(|import| {
            if import.module != file.module {
                return Some(import);
            }
            // A self-module symbol: redirected to the companion when this file is
            // split off from its types, otherwise dropped.
            companion.map(|module| Import {
                module: module.to_string(),
                imported: import.imported,
            })
        })
        .collect()
}

fn walk_decl(
    decl: &Decl,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    match decl {
        Decl::Interface(interface) => walk_fields(&interface.fields, acc, visited),
        Decl::Union(union) => {
            for variant in &union.variants {
                walk_fields(&variant.fields, acc, visited);
                walk_opt_type(variant.payload.as_ref(), acc, visited);
            }
        }
        Decl::Method(method) => walk_signature(&method.params, method.ret.as_ref(), acc, visited),
        Decl::Function(function) => {
            walk_signature(&function.params, function.ret.as_ref(), acc, visited);
            walk_body(&function.body, acc, visited);
        }
        // A raw item's text is opaque, but its declared refs are collected so
        // anything it references is still imported.
        Decl::Raw(raw) => walk_refs(&raw.refs, acc, visited),
        // Enum members and the enum's own name are identifiers, and an alias's
        // definition is opaque text: neither contributes imports.
        Decl::Enum(_) | Decl::Alias(_) => {}
    }
}

fn walk_refs(
    refs: &[Symbol],
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    for symbol in refs {
        collect_symbol(symbol, acc, visited);
    }
}

fn walk_fields(
    fields: &[Field],
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    for field in fields {
        walk_type(&field.ty, acc, visited);
    }
}

fn walk_signature(
    params: &[Field],
    ret: Option<&TypeExpr>,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    walk_fields(params, acc, visited);
    walk_opt_type(ret, acc, visited);
}

fn walk_opt_type(
    ty: Option<&TypeExpr>,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    if let Some(ty) = ty {
        walk_type(ty, acc, visited);
    }
}

// A function body's statements are opaque text, but the symbols they reference
// are declared so their imports are still collected.
fn walk_body(
    body: &FnBody,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    match body {
        FnBody::Raw { refs, .. } => walk_refs(refs, acc, visited),
    }
}

fn walk_type(
    ty: &TypeExpr,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    match ty {
        TypeExpr::Ref(symbol) => collect_symbol(symbol, acc, visited),
        TypeExpr::List(inner) | TypeExpr::Nullable(inner) => walk_type(inner, acc, visited),
        TypeExpr::Map(key, value) | TypeExpr::Entries(key, value) => {
            walk_type(key, acc, visited);
            walk_type(value, acc, visited);
        }
        TypeExpr::Generic(symbol, args) => {
            collect_symbol(symbol, acc, visited);
            for arg in args {
                walk_type(arg, acc, visited);
            }
        }
    }
}

fn collect_symbol(
    symbol: &Symbol,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    let key = (symbol.name.clone(), symbol.import.clone());
    if !visited.insert(key) {
        // Already processed: its import is collected and its references walked.
        return;
    }
    if let Some(import) = &symbol.import {
        acc.insert(import.clone());
    }
    for reference in &symbol.references {
        collect_symbol(reference, acc, visited);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::tree::{
        Decl, EnumDecl, Field, FnBody, Function, Interface, Method, Raw, UnionDecl, Variant,
    };

    fn field(name: &str, ty: TypeExpr) -> Field {
        Field {
            name: Symbol::builtin(name),
            ty,
            nullable: false,
            wire: None,
        }
    }

    fn interface_file(module: &str, fields: Vec<Field>) -> File {
        File {
            module: module.into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Subject"),
                fields,
            })],
        }
    }

    #[test]
    fn a_referenced_symbol_emits_its_import_once() {
        let file = interface_file(
            "billing",
            vec![field(
                "method",
                TypeExpr::Ref(Symbol::imported(
                    "PaymentMethod",
                    "payments",
                    "PaymentMethod",
                )),
            )],
        );
        assert_eq!(
            collect(&file),
            vec![Import {
                module: "payments".into(),
                imported: "PaymentMethod".into(),
            }]
        );
    }

    #[test]
    fn two_fields_referencing_the_same_symbol_do_not_duplicate() {
        let charge = || TypeExpr::Ref(Symbol::imported("Charge", "payments", "Charge"));
        let file = interface_file("billing", vec![field("a", charge()), field("b", charge())]);
        assert_eq!(collect(&file).len(), 1);
    }

    #[test]
    fn builtins_contribute_no_imports() {
        let file = interface_file(
            "billing",
            vec![field("id", TypeExpr::Ref(Symbol::builtin("string")))],
        );
        assert!(collect(&file).is_empty());
    }

    #[test]
    fn generic_collects_head_and_args_transitively_across_modules() {
        // Page<Charge>: Page from core, Charge from payments. Both modeled two
        // ways to prove the fold reaches the argument either via Generic args or
        // via Symbol.references.
        let via_args = TypeExpr::Generic(
            Symbol::imported("Page", "core", "Page"),
            vec![TypeExpr::Ref(Symbol::imported(
                "Charge", "payments", "Charge",
            ))],
        );
        let via_references = TypeExpr::Ref(
            Symbol::imported("Page", "core", "Page")
                .referencing(vec![Symbol::imported("Charge", "payments", "Charge")]),
        );
        let expected = vec![
            Import {
                module: "core".into(),
                imported: "Page".into(),
            },
            Import {
                module: "payments".into(),
                imported: "Charge".into(),
            },
        ];
        assert_eq!(
            collect(&interface_file("billing", vec![field("p", via_args)])),
            expected
        );
        assert_eq!(
            collect(&interface_file("billing", vec![field("p", via_references)])),
            expected
        );
    }

    #[test]
    fn self_module_imports_are_dropped() {
        // A symbol whose import points back at the file's own module needs no
        // import statement.
        let file = interface_file(
            "payments",
            vec![field(
                "self_ref",
                TypeExpr::Ref(Symbol::imported("Charge", "payments", "Charge")),
            )],
        );
        assert!(collect(&file).is_empty());
    }

    #[test]
    fn ordering_is_deterministic_by_module_then_imported() {
        let file = interface_file(
            "billing",
            vec![
                field("z", TypeExpr::Ref(Symbol::imported("Z", "zeta", "Z"))),
                field("a", TypeExpr::Ref(Symbol::imported("A", "alpha", "A"))),
                field("b", TypeExpr::Ref(Symbol::imported("B", "alpha", "B"))),
            ],
        );
        let modules: Vec<_> = collect(&file)
            .into_iter()
            .map(|i| (i.module, i.imported))
            .collect();
        assert_eq!(
            modules,
            vec![
                ("alpha".into(), "A".into()),
                ("alpha".into(), "B".into()),
                ("zeta".into(), "Z".into()),
            ]
        );
    }

    #[test]
    fn list_map_and_nullable_descend_into_their_children() {
        let file = interface_file(
            "billing",
            vec![field(
                "nested",
                TypeExpr::list(TypeExpr::map(
                    TypeExpr::Ref(Symbol::imported("Key", "k", "Key")),
                    TypeExpr::nullable(TypeExpr::Ref(Symbol::imported("Val", "v", "Val"))),
                )),
            )],
        );
        assert_eq!(collect(&file).len(), 2);
    }

    #[test]
    fn an_entries_type_collects_both_key_and_value_imports() {
        let file = interface_file(
            "billing",
            vec![field(
                "counts",
                TypeExpr::entries(
                    TypeExpr::Ref(Symbol::imported("Key", "k", "Key")),
                    TypeExpr::Ref(Symbol::imported("Val", "v", "Val")),
                ),
            )],
        );
        assert_eq!(collect(&file).len(), 2);
    }

    #[test]
    fn reference_cycles_terminate() {
        // Two symbols that reference each other. The visited guard stops the
        // recursion; both imports are still collected.
        let mut a = Symbol::imported("A", "ma", "A");
        let b = Symbol::imported("B", "mb", "B").referencing(vec![a.clone()]);
        a.references = vec![b];
        let file = interface_file("billing", vec![field("a", TypeExpr::Ref(a))]);
        assert_eq!(collect(&file).len(), 2);
    }

    #[test]
    fn methods_collect_from_params_and_return_and_enums_contribute_nothing() {
        let file = File {
            module: "billing".into(),
            decls: vec![
                Decl::Enum(EnumDecl {
                    name: Symbol::builtin("Status"),
                    members: vec![Symbol::builtin("Active")],
                    backing: crate::codegen::tree::EnumRepr::String,
                }),
                Decl::Method(Method {
                    name: Symbol::builtin("create"),
                    params: vec![field(
                        "input",
                        TypeExpr::Ref(Symbol::imported("In", "req", "In")),
                    )],
                    ret: Some(TypeExpr::Ref(Symbol::imported("Out", "resp", "Out"))),
                }),
                Decl::Method(Method {
                    name: Symbol::builtin("ping"),
                    params: vec![],
                    ret: None,
                }),
            ],
        };
        assert_eq!(collect(&file).len(), 2);
    }

    #[test]
    fn function_signature_and_body_refs_contribute_imports() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Function(Function {
                name: Symbol::builtin("decode"),
                params: vec![field(
                    "raw",
                    TypeExpr::Ref(Symbol::imported("In", "req", "In")),
                )],
                ret: Some(TypeExpr::Ref(Symbol::imported("Out", "resp", "Out"))),
                body: FnBody::Raw {
                    text: "return helper(raw);".into(),
                    refs: vec![Symbol::imported("helper", "codecs", "helper")],
                },
            })],
        };
        // param type, return type, and the body-referenced symbol.
        assert_eq!(collect(&file).len(), 3);
    }

    #[test]
    fn a_raw_decls_refs_contribute_imports_while_its_text_stays_opaque() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Raw(Raw {
                // The text mentions a type, but only the declared refs are walked.
                text: "impl Charge { fn touch(&self) -> Helper { Helper } }".into(),
                refs: vec![Symbol::imported("Helper", "helpers", "Helper")],
            })],
        };
        assert_eq!(
            collect(&file),
            vec![Import {
                module: "helpers".into(),
                imported: "Helper".into(),
            }]
        );
    }

    #[test]
    fn union_variant_fields_and_payloads_contribute_imports() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Union(UnionDecl {
                name: Symbol::builtin("Method"),
                discriminator: "type".into(),
                variants: vec![Variant {
                    name: Symbol::builtin("Card"),
                    fields: vec![field(
                        "brand",
                        TypeExpr::Ref(Symbol::imported("Brand", "cards", "Brand")),
                    )],
                    payload: Some(TypeExpr::Ref(Symbol::imported(
                        "CardData", "cards", "CardData",
                    ))),
                    wire: None,
                }],
            })],
        };
        // The variant field's type and the variant payload's type.
        assert_eq!(collect(&file).len(), 2);
    }
}

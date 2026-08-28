//! Deterministic, transitive import collection.
//!
//! A file's import set is never written by hand: it is folded from the symbols
//! reachable from the file's declarations. Walking gathers each referenced
//! symbol's import, recurses into the symbol's `references` (so a generic
//! `Page<Charge>` pulls both `Page` and `Charge`), deduplicates, and orders
//! deterministically so the output is byte-stable across runs.
//!
//! What a collected import then *points at* is the [`Resolver`]'s call. A symbol
//! table can only say which IR module a type belongs to; which file of that
//! module declares it is decided later, when the module is split into emission
//! groups. The resolver closes that gap: it re-points each import at the group
//! that declares the symbol, and drops the ones that turn out to be in the
//! importing file's own compilation unit.

use std::collections::{BTreeSet, HashSet};

use crate::codegen::symbol::{Import, Symbol};
use crate::codegen::tree::{Decl, Field, File, FnBody, TypeExpr};

/// How a collected reference becomes (or stops being) an import statement.
pub trait Resolver {
    /// The import the file at `from` needs for `import`, or `None` when the
    /// symbol is already in that file's compilation unit.
    fn resolve(&self, from: &str, import: &Import) -> Option<Import>;
}

/// The resolver for a file that is a module unto itself: a reference back to the
/// file's own module needs no import, and everything else is kept verbatim.
pub struct SelfModule;

impl Resolver for SelfModule {
    fn resolve(&self, from: &str, import: &Import) -> Option<Import> {
        (import.module != from).then(|| import.clone())
    }
}

/// Collect the deduplicated, deterministically-ordered import set of a file,
/// dropping references back into the file's own module.
///
/// Determinism comes from a `BTreeSet`, which orders imports by `(module,
/// imported)` regardless of discovery order.
pub fn collect(file: &File) -> Vec<Import> {
    collect_with(file, &SelfModule)
}

/// Like [`collect`], but each reference is put through `resolver`, which decides
/// what it points at and whether it survives at all.
pub fn collect_with(file: &File, resolver: &dyn Resolver) -> Vec<Import> {
    let mut acc: BTreeSet<Import> = BTreeSet::new();
    // Guards against reference cycles and skips re-walking a symbol already
    // seen. Keyed on (name, import) so two distinct same-named symbols from
    // different modules are still both visited.
    let mut visited: HashSet<(String, Option<Import>)> = HashSet::new();
    for decl in &file.decls {
        walk_decl(decl, &mut acc, &mut visited);
    }
    // Resolving can map two references onto one import (two groups collapsing
    // into one Go package), so the result is re-ordered and deduplicated.
    acc.iter()
        .filter_map(|import| resolver.resolve(&file.module, import))
        .collect::<BTreeSet<Import>>()
        .into_iter()
        .collect()
}

/// Apply `f` to every symbol the file's declarations reference, including the
/// transitive `references` a symbol carries.
///
/// This is what re-points a file's references from the IR module a symbol table
/// knows to the group that actually declares them. It has to happen on the tree
/// rather than on the collected import set, because a target also reads a
/// symbol's import when rendering the reference itself (Go qualifies a
/// cross-package name with the package's selector).
pub fn repoint(file: &mut File, f: &dyn Fn(&mut Symbol)) {
    for decl in &mut file.decls {
        match decl {
            Decl::Interface(interface) => repoint_fields(&mut interface.fields, f),
            Decl::Union(union) => {
                for variant in &mut union.variants {
                    repoint_fields(&mut variant.fields, f);
                    if let Some(ty) = &mut variant.payload {
                        repoint_type(ty, f);
                    }
                }
            }
            Decl::Method(method) => repoint_method(method, f),
            Decl::Client(client) => {
                for method in &mut client.methods {
                    repoint_method(method, f);
                }
            }
            Decl::Function(function) => {
                repoint_fields(&mut function.params, f);
                if let Some(ty) = &mut function.ret {
                    repoint_type(ty, f);
                }
                let FnBody::Raw { refs, .. } = &mut function.body;
                repoint_symbols(refs, f);
            }
            Decl::Raw(raw) => repoint_symbols(&mut raw.refs, f),
            Decl::Enum(_) | Decl::Alias(_) => {}
        }
    }
}

fn repoint_method(method: &mut crate::codegen::tree::Method, f: &dyn Fn(&mut Symbol)) {
    repoint_fields(&mut method.params, f);
    for ty in method.ret.iter_mut().chain(method.err.iter_mut()) {
        repoint_type(ty, f);
    }
}

fn repoint_fields(fields: &mut [Field], f: &dyn Fn(&mut Symbol)) {
    for field in fields {
        repoint_type(&mut field.ty, f);
    }
}

fn repoint_symbols(symbols: &mut [Symbol], f: &dyn Fn(&mut Symbol)) {
    for symbol in symbols {
        f(symbol);
        repoint_symbols(&mut symbol.references, f);
    }
}

fn repoint_type(ty: &mut TypeExpr, f: &dyn Fn(&mut Symbol)) {
    match ty {
        TypeExpr::Ref(symbol) => repoint_symbols(std::slice::from_mut(symbol), f),
        TypeExpr::List(inner) | TypeExpr::Nullable(inner) => repoint_type(inner, f),
        TypeExpr::Map(key, value) | TypeExpr::Entries(key, value) => {
            repoint_type(key, f);
            repoint_type(value, f);
        }
        TypeExpr::Generic(symbol, args) => {
            repoint_symbols(std::slice::from_mut(symbol), f);
            for arg in args {
                repoint_type(arg, f);
            }
        }
    }
}

/// The symbols a file declares, so the pass that builds the symbol-to-group index
/// knows which group defines what. A raw item's text is opaque, so what it
/// declares is carried alongside it by the emitter rather than inferred here.
pub fn declared_symbols(decls: &[Decl]) -> Vec<String> {
    decls
        .iter()
        .flat_map(|decl| match decl {
            Decl::Interface(x) => vec![x.name.name.clone()],
            Decl::Enum(x) => vec![x.name.name.clone()],
            Decl::Union(x) => vec![x.name.name.clone()],
            Decl::Method(x) => vec![x.name.name.clone()],
            Decl::Function(x) => vec![x.name.name.clone()],
            Decl::Alias(x) => vec![x.name.name.clone()],
            Decl::Client(x) => vec![x.name.name.clone()],
            Decl::Raw(x) => x.provides.clone(),
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
        Decl::Method(method) => walk_method(method, acc, visited),
        Decl::Client(client) => {
            for method in &client.methods {
                walk_method(method, acc, visited);
            }
        }
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

// A method's error channel carries a type of its own (a `Result` error, say),
// so it contributes imports exactly like the return type.
fn walk_method(
    method: &crate::codegen::tree::Method,
    acc: &mut BTreeSet<Import>,
    visited: &mut HashSet<(String, Option<Import>)>,
) {
    walk_signature(&method.params, method.ret.as_ref(), acc, visited);
    walk_opt_type(method.err.as_ref(), acc, visited);
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
            tag: None,
            name: Symbol::builtin(name),
            ty,
            nullable: false,
            wire: None,
            deprecated: None,
            doc: None,
        }
    }

    fn method(
        name: &str,
        params: Vec<Field>,
        ret: Option<TypeExpr>,
        err: Option<TypeExpr>,
    ) -> Method {
        Method {
            name: Symbol::builtin(name),
            params,
            ret,
            err,
            is_async: false,
            doc: None,
        }
    }

    fn imported_ref(name: &str, module: &str) -> TypeExpr {
        TypeExpr::Ref(Symbol::imported(name, module, name))
    }

    fn interface_file(module: &str, fields: Vec<Field>) -> File {
        File {
            module: module.into(),
            decls: vec![Decl::Interface(Interface {
                name: Symbol::builtin("Subject"),
                params: vec![],
                fields,
                deprecated: None,
                doc: None,
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
                    member_docs: vec![None],
                    backing: crate::codegen::tree::EnumRepr::String,
                    deprecated: None,
                    doc: None,
                }),
                Decl::Method(method(
                    "create",
                    vec![field("input", imported_ref("In", "req"))],
                    Some(imported_ref("Out", "resp")),
                    None,
                )),
                Decl::Method(method("ping", vec![], None, None)),
            ],
        };
        assert_eq!(collect(&file).len(), 2);
    }

    #[test]
    fn a_client_collects_params_returns_and_error_channels() {
        let file = File {
            module: "billing".into(),
            decls: vec![Decl::Client(crate::codegen::tree::ClientDecl {
                name: Symbol::builtin("Client"),
                methods: vec![method(
                    "create",
                    vec![field("input", imported_ref("In", "req"))],
                    Some(imported_ref("Out", "resp")),
                    Some(imported_ref("Err", "errs")),
                )],
            })],
        };
        // The param type, the return type, and the error-channel type.
        assert_eq!(collect(&file).len(), 3);
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
                ..Raw::default()
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
                    doc: None,
                }],
                deprecated: None,
                doc: None,
            })],
        };
        // The variant field's type and the variant payload's type.
        assert_eq!(collect(&file).len(), 2);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::codegen::tree::Interface;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use proptest::sample::select;

    // An imported (module, name) drawn from a small pool; the pool includes the
    // file's own module so the self-module filter is exercised.
    fn imported_pair() -> impl Strategy<Value = (String, String)> {
        (
            select(vec!["alpha", "zeta", "billing", "payments"]).prop_map(String::from),
            select(vec!["A", "B", "C", "Charge"]).prop_map(String::from),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(500))]

        /// For any set of referenced symbols, the collected imports are strictly
        /// ordered by (module, imported), carry no duplicates, and drop the file's
        /// own module.
        #[test]
        fn imports_are_sorted_deduped_and_exclude_self_module(
            refs in vec(imported_pair(), 0..8)
        ) {
            let module = "billing";
            let fields: Vec<Field> = refs
                .iter()
                .enumerate()
                .map(|(i, (m, n))| Field {
                    tag: None,
                    name: Symbol::builtin(format!("f{i}")),
                    ty: TypeExpr::Ref(Symbol::imported(n, m, n)),
                    nullable: false,
                    wire: None,
                    deprecated: None,
                    doc: None,
                })
                .collect();
            let file = File {
                module: module.into(),
                decls: vec![Decl::Interface(Interface {
                    name: Symbol::builtin("Subject"),
                    params: vec![],
                    fields,
                    deprecated: None,
                    doc: None,
                })],
            };

            let got = collect(&file);
            // Strictly ascending by (module, imported): both sorted and deduped.
            for w in got.windows(2) {
                prop_assert!(
                    (w[0].module.as_str(), w[0].imported.as_str())
                        < (w[1].module.as_str(), w[1].imported.as_str())
                );
            }
            // The result is exactly the sorted-unique set of non-self imports.
            let mut expected: Vec<(String, String)> = refs
                .iter()
                .filter(|(m, _)| m != module)
                .cloned()
                .collect();
            expected.sort();
            expected.dedup();
            let got_pairs: Vec<(String, String)> = got
                .iter()
                .map(|i| (i.module.clone(), i.imported.clone()))
                .collect();
            prop_assert_eq!(got_pairs, expected);
        }
    }
}

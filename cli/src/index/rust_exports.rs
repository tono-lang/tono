//! The Rust extractor's second pass: what the crate exports, resolved from
//! the module tree by following `pub use` until nothing new appears, then
//! turned into index symbols named the way a spelling reaches them
//! (`Name` at the root, `module::Name` below it).
//!
//! Re-exports are a fixpoint because they chain: `pub use inner::*` in a
//! module that another module glob-imports needs the inner module settled
//! first. Every step only adds bindings, so the loop ends when a full pass
//! adds none; a pass count bounded by the module count catches a runaway.

use std::collections::{BTreeMap, BTreeSet};

use super::format::{Member, MemberKind, Symbol, SymbolKind};
use super::rust_walk::{Item, Method, ModTree, Use};

/// A name bound in a module's export table: an item of some module, or a
/// module reachable under that name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Binding {
    Item { module: Vec<String>, name: String },
    Module(Vec<String>),
}

type Exports = BTreeMap<Vec<String>, BTreeMap<String, Binding>>;

fn module_at<'a>(tree: &'a ModTree, path: &[String]) -> Option<&'a ModTree> {
    path.iter().try_fold(tree, |m, seg| m.mods.get(seg))
}

fn each_module<'a>(
    tree: &'a ModTree,
    path: Vec<String>,
    out: &mut Vec<(Vec<String>, &'a ModTree)>,
) {
    for (name, child) in &tree.mods {
        let mut p = path.clone();
        p.push(name.clone());
        each_module(child, p, out);
    }
    out.push((path, tree));
}

/// The export tables every module starts with: its own public items and
/// public child modules.
fn own_exports(tree: &ModTree) -> Exports {
    let mut modules = Vec::new();
    each_module(tree, Vec::new(), &mut modules);
    modules
        .into_iter()
        .map(|(path, m)| {
            let mut table = BTreeMap::new();
            for item in &m.items {
                if let Some(name) = item.name() {
                    table.insert(
                        name.to_string(),
                        Binding::Item {
                            module: path.clone(),
                            name: name.to_string(),
                        },
                    );
                }
            }
            for name in &m.pub_mods {
                let mut p = path.clone();
                p.push(name.clone());
                table.insert(name.clone(), Binding::Module(p));
            }
            (path, table)
        })
        .collect()
}

/// The module a `use` path's prefix names, from module `from`. Segments
/// resolve through the tree (private modules included: a `pub use` reaches
/// into them) and through re-exported modules already in the tables.
fn resolve_module(
    tree: &ModTree,
    exports: &Exports,
    from: &[String],
    segments: &[String],
) -> Option<Vec<String>> {
    let mut current: Vec<String> = from.to_vec();
    let mut rest = segments;
    if let Some(first) = segments.first() {
        match first.as_str() {
            "crate" => {
                current = Vec::new();
                rest = &segments[1..];
            }
            "self" => rest = &segments[1..],
            "super" => {
                current.pop();
                rest = &segments[1..];
            }
            _ => {}
        }
    }
    for seg in rest {
        if seg == "super" {
            current.pop();
            continue;
        }
        let here = module_at(tree, &current)?;
        if here.mods.contains_key(seg) {
            current.push(seg.clone());
        } else {
            match exports.get(&current).and_then(|t| t.get(seg)) {
                Some(Binding::Module(path)) => current = path.clone(),
                _ => return None,
            }
        }
    }
    Some(current)
}

/// Apply one `use` to module `from`'s table. `true` when it bound something
/// new.
fn apply_use(tree: &ModTree, exports: &mut Exports, from: &[String], u: &Use) -> bool {
    if u.glob {
        let Some(target) = resolve_module(tree, exports, from, &u.path) else {
            return false;
        };
        let incoming: Vec<(String, Binding)> = exports
            .get(&target)
            .map(|t| t.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        let table = exports.entry(from.to_vec()).or_default();
        let mut added = false;
        for (name, binding) in incoming {
            if let std::collections::btree_map::Entry::Vacant(slot) = table.entry(name) {
                slot.insert(binding);
                added = true;
            }
        }
        return added;
    }
    let Some((last, prefix)) = u.path.split_last() else {
        return false;
    };
    let binding = if last == "self" {
        resolve_module(tree, exports, from, prefix).map(Binding::Module)
    } else {
        let Some(target) = resolve_module(tree, exports, from, prefix) else {
            return false;
        };
        match exports.get(&target).and_then(|t| t.get(last)) {
            Some(b) => Some(b.clone()),
            None if module_at(tree, &target).is_some_and(|m| m.mods.contains_key(last)) => {
                let mut p = target.clone();
                p.push(last.clone());
                Some(Binding::Module(p))
            }
            None => None,
        }
    };
    let Some(binding) = binding else {
        return false;
    };
    let name = u.alias.clone().unwrap_or_else(|| {
        if last == "self" {
            prefix.last().cloned().unwrap_or_default()
        } else {
            last.clone()
        }
    });
    let table = exports.entry(from.to_vec()).or_default();
    if table.get(&name) == Some(&binding) {
        return false;
    }
    table.insert(name, binding);
    true
}

/// Every module's exports once the re-exports have settled, and the notes
/// the resolution produced.
fn exports(tree: &ModTree) -> (Exports, Vec<String>) {
    let mut exports = own_exports(tree);
    let mut modules = Vec::new();
    each_module(tree, Vec::new(), &mut modules);
    let cap = modules.len() + 2;
    let mut notes = Vec::new();
    let mut passes = 0;
    loop {
        let mut added = false;
        for (path, m) in &modules {
            for u in &m.uses {
                added |= apply_use(tree, &mut exports, path, u);
            }
        }
        passes += 1;
        if !added {
            break;
        }
        if passes >= cap {
            notes.push("re-exports did not settle; some may be missing".to_string());
            break;
        }
    }
    // A `use` that never bound anything names another crate (or a path the
    // walk did not read): said once.
    let unresolved: BTreeSet<String> = modules
        .iter()
        .flat_map(|(path, m)| m.uses.iter().map(move |u| (path, u)))
        .filter(|(path, u)| {
            let prefix: &[String] = if u.glob {
                &u.path
            } else {
                &u.path[..u.path.len().saturating_sub(1)]
            };
            resolve_module(tree, &exports, path, prefix).is_none()
        })
        .map(|(_, u)| u.path.first().cloned().unwrap_or_default())
        .collect();
    if !unresolved.is_empty() {
        notes.push(format!(
            "re-exports from {} are not indexed",
            unresolved.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    (exports, notes)
}

fn member(m: &Method) -> Member {
    Member {
        name: m.name.clone(),
        kind: MemberKind::Method,
        is_static: m.is_static,
        signatures: vec![m.sig.clone()],
    }
}

/// The impl blocks of the crate, by the type name they attach to; a trait
/// impl only when the trait is the crate's own (a foreign trait's methods
/// are the foreign crate's API, not this one's).
fn impls_by_type<'a>(
    tree: &'a ModTree,
    local_traits: &BTreeSet<String>,
) -> BTreeMap<&'a str, Vec<&'a Method>> {
    let mut modules = Vec::new();
    each_module(tree, Vec::new(), &mut modules);
    let mut out: BTreeMap<&str, Vec<&Method>> = BTreeMap::new();
    for (_, m) in modules {
        for item in &m.items {
            if let Item::Impl {
                self_ty,
                trait_,
                methods,
            } = item
            {
                if trait_.as_ref().is_some_and(|t| !local_traits.contains(t)) {
                    continue;
                }
                out.entry(self_ty).or_default().extend(methods);
            }
        }
    }
    out
}

fn symbol_of(name: String, item: &Item, impls: &BTreeMap<&str, Vec<&Method>>) -> Symbol {
    let methods = |type_name: &str| -> Vec<Member> {
        impls
            .get(type_name)
            .map(|ms| ms.iter().map(|m| member(m)).collect())
            .unwrap_or_default()
    };
    match item {
        Item::Fn { sig, doc, .. } => Symbol {
            name,
            kind: SymbolKind::Function,
            signatures: vec![sig.clone()],
            doc: doc.clone(),
            members: vec![],
        },
        Item::Struct {
            name: own,
            generics,
            fields,
            doc,
        } => {
            let mut members: Vec<Member> = fields
                .iter()
                .map(|(f, ty)| Member {
                    name: f.clone(),
                    kind: MemberKind::Field,
                    is_static: false,
                    signatures: vec![ty.clone()],
                })
                .collect();
            members.extend(methods(own));
            Symbol {
                name,
                kind: SymbolKind::Struct,
                signatures: if generics.is_empty() {
                    vec![]
                } else {
                    vec![format!("{own}{generics}")]
                },
                doc: doc.clone(),
                members,
            }
        }
        Item::Enum {
            name: own,
            generics,
            variants,
            doc,
        } => {
            let mut members: Vec<Member> = variants
                .iter()
                .map(|(v, payload)| Member {
                    name: v.clone(),
                    kind: MemberKind::Const,
                    is_static: true,
                    signatures: if payload.is_empty() {
                        vec![]
                    } else {
                        vec![payload.clone()]
                    },
                })
                .collect();
            members.extend(methods(own));
            Symbol {
                name,
                kind: SymbolKind::Enum,
                signatures: if generics.is_empty() {
                    vec![]
                } else {
                    vec![format!("{own}{generics}")]
                },
                doc: doc.clone(),
                members,
            }
        }
        Item::Trait { methods, doc, .. } => Symbol {
            name,
            kind: SymbolKind::Trait,
            signatures: vec![],
            doc: doc.clone(),
            members: methods.iter().map(member).collect(),
        },
        Item::TypeAlias { target, doc, .. } => Symbol {
            name,
            kind: SymbolKind::Type,
            signatures: vec![target.clone()],
            doc: doc.clone(),
            members: vec![],
        },
        Item::Const { ty, doc, .. } => Symbol {
            name,
            kind: SymbolKind::Const,
            signatures: vec![ty.clone()],
            doc: doc.clone(),
            members: vec![],
        },
        Item::Impl { .. } => unreachable!("an impl block is never bound by name"),
    }
}

/// The crate's public API as index symbols, with the notes the read
/// produced: the macro edge, the re-exports it could not follow, the module
/// files it did not find.
pub(crate) fn public_api(tree: &ModTree) -> (Vec<Symbol>, Vec<String>) {
    let (exports, mut notes) = exports(tree);
    let mut modules = Vec::new();
    each_module(tree, Vec::new(), &mut modules);
    let local_traits: BTreeSet<String> = modules
        .iter()
        .flat_map(|(_, m)| m.items.iter())
        .filter_map(|i| match i {
            Item::Trait { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect();
    let impls = impls_by_type(tree, &local_traits);
    let macro_items: usize = modules.iter().map(|(_, m)| m.macro_items).sum();
    for (_, m) in &modules {
        notes.extend(m.notes.iter().cloned());
    }
    if macro_items > 0 {
        notes.insert(
            0,
            format!("{macro_items} macro items seen; what a macro produces is not indexed"),
        );
    }
    // Walk the exported module graph from the root, naming each module by
    // the path a spelling would write; a module reachable two ways is
    // listed under the shorter path first and once per path.
    let mut symbols = Vec::new();
    let mut queue: Vec<(Vec<String>, Vec<String>)> = vec![(Vec::new(), Vec::new())];
    let mut seen: BTreeSet<Vec<String>> = BTreeSet::new();
    while let Some((spelled, module)) = queue.first().cloned() {
        queue.remove(0);
        if !seen.insert(module.clone()) {
            continue;
        }
        let Some(table) = exports.get(&module) else {
            continue;
        };
        for (name, binding) in table {
            let mut path = spelled.clone();
            path.push(name.clone());
            match binding {
                Binding::Module(target) => {
                    symbols.push(Symbol {
                        name: path.join("::"),
                        kind: SymbolKind::Namespace,
                        signatures: vec![],
                        doc: String::new(),
                        members: vec![],
                    });
                    queue.push((path, target.clone()));
                }
                Binding::Item {
                    module: home,
                    name: item_name,
                } => {
                    if let Some(item) = module_at(tree, home)
                        .and_then(|m| m.items.iter().find(|i| i.name() == Some(item_name)))
                    {
                        symbols.push(symbol_of(path.join("::"), item, &impls));
                    }
                }
            }
        }
    }
    (symbols, notes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::rust_walk::{load_crate, tests::fixture_crate};

    fn api(name: &str) -> (Vec<Symbol>, Vec<String>) {
        let dir = fixture_crate(name);
        public_api(&load_crate(&dir.join("src/lib.rs")).unwrap())
    }

    #[test]
    fn root_names_are_bare_and_re_exports_reach_private_modules() {
        let (symbols, _) = api("exports");
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        for expected in [
            "open",
            "Options",
            "Error",
            "Run",
            "Mode",
            "Pair",
            "LIMIT",
            "Hidden",
            "Meter",
            "a",
            "a::Dial",
            "a::deep",
            "a::deep::deepest",
            "c",
            "c::Dial",
            "c::c_fn",
            "c::b",
            "c::b::Hidden",
            "c::b::unreachable_fn",
            "d",
            "d::moved",
            "e",
            "e::inline_fn",
        ] {
            assert!(
                names.contains(&expected),
                "{expected} missing from {names:?}"
            );
        }
        for absent in [
            "nope",
            "b",
            "b::Hidden",
            "e::hidden",
            "only_in_tests",
            "Foreign",
            "a::deep::deep",
        ] {
            assert!(!names.contains(&absent), "{absent} present in {names:?}");
        }
    }

    #[test]
    fn members_come_from_fields_inherent_impls_and_local_traits() {
        let (symbols, _) = api("members");
        let by = |n: &str| symbols.iter().find(|s| s.name == n).unwrap();
        let options = by("Options");
        assert_eq!(options.kind, SymbolKind::Struct);
        let members: Vec<(&str, MemberKind, bool)> = options
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.kind, m.is_static))
            .collect();
        assert_eq!(
            members,
            vec![
                ("precision", MemberKind::Field, false),
                ("new", MemberKind::Method, true),
                ("get", MemberKind::Method, false),
                ("run", MemberKind::Method, false),
                ("make", MemberKind::Method, true),
            ]
        );
        assert_eq!(options.members[1].signatures, vec!["fn new() -> Self"]);
        let meter = by("Meter");
        assert_eq!(meter.kind, SymbolKind::Struct);
        assert_eq!(meter.signatures, vec!["Dial<T>"]);
        assert_eq!(meter.doc, "A dial.");
        assert_eq!(meter.members.len(), 2);
        assert_eq!(meter.members[1].name, "read");
        let mode = by("Mode");
        assert_eq!(mode.kind, SymbolKind::Enum);
        let variants: Vec<(&str, Vec<String>)> = mode
            .members
            .iter()
            .map(|m| (m.name.as_str(), m.signatures.clone()))
            .collect();
        assert_eq!(
            variants,
            vec![
                ("Fast", vec![]),
                ("Slow", vec!["(u8)".to_string()]),
                ("Custom", vec!["{ level: u8 }".to_string()]),
            ]
        );
        let run = by("Run");
        assert_eq!(run.kind, SymbolKind::Trait);
        assert!(run.members[1].is_static);
        assert_eq!(by("Pair").signatures, vec!["(T, T)"]);
        assert_eq!(by("LIMIT").kind, SymbolKind::Const);
        assert_eq!(by("a").kind, SymbolKind::Namespace);
        assert_eq!(
            by("open").signatures[0],
            "fn open(name: &str, opts: Option<&Options>) -> Result<Dial, Error>"
        );
    }

    #[test]
    fn the_notes_name_the_macro_edge_and_what_was_not_followed() {
        let (_, notes) = api("notes");
        assert!(notes[0].contains("1 macro items seen"), "{notes:?}");
        assert!(
            notes
                .iter()
                .any(|n| n.contains("re-exports from other_crate are not indexed")),
            "{notes:?}"
        );
        assert!(
            notes
                .iter()
                .any(|n| n.contains("module missing was not found")),
            "{notes:?}"
        );
    }

    #[test]
    fn a_glob_cycle_settles_without_running_away() {
        let dir =
            std::env::temp_dir().join(format!("tono-index-rust-{}-glob-cycle", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(
            dir.join("src/lib.rs"),
            "pub mod x { pub use crate::y::*; pub fn fx() {} }\npub mod y { pub use crate::x::*; pub fn fy() {} }\npub use x::*;\n",
        )
        .unwrap();
        let (symbols, notes) = public_api(&load_crate(&dir.join("src/lib.rs")).unwrap());
        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"fx") && names.contains(&"fy"), "{names:?}");
        assert!(
            names.contains(&"x::fy") && names.contains(&"y::fx"),
            "{names:?}"
        );
        assert!(notes.is_empty(), "{notes:?}");
    }
}

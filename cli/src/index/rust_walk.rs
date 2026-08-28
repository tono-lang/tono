//! The Rust extractor's first pass: the crate's source files parsed with
//! syn into a tree of modules, each holding its public items, its `use`
//! declarations and its child modules. Nothing is resolved here; the second
//! pass (`rust_exports`) decides what is reachable from the crate root.
//!
//! A syntactic read has a known edge: an item a macro produces is not in
//! the source, so it is not in the index. The tree counts what it saw of
//! macros so the note can say so; the index is a suggestion, and a missing
//! suggestion is the harmless side of that edge.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use quote::ToTokens;

/// One method of an impl block or a trait.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Method {
    pub name: String,
    pub sig: String,
    /// No `self` receiver: called on the type, not on a value.
    pub is_static: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Item {
    Fn {
        name: String,
        sig: String,
        doc: String,
    },
    Struct {
        name: String,
        generics: String,
        fields: Vec<(String, String)>,
        doc: String,
    },
    Enum {
        name: String,
        generics: String,
        variants: Vec<(String, String)>,
        doc: String,
    },
    Trait {
        name: String,
        methods: Vec<Method>,
        doc: String,
    },
    TypeAlias {
        name: String,
        target: String,
        doc: String,
    },
    Const {
        name: String,
        ty: String,
        doc: String,
    },
    /// An impl block, wherever it sits: attached to its type by name in the
    /// second pass. `trait_` is the implemented trait's last path segment.
    Impl {
        self_ty: String,
        trait_: Option<String>,
        methods: Vec<Method>,
    },
}

impl Item {
    pub(crate) fn name(&self) -> Option<&str> {
        match self {
            Item::Fn { name, .. }
            | Item::Struct { name, .. }
            | Item::Enum { name, .. }
            | Item::Trait { name, .. }
            | Item::TypeAlias { name, .. }
            | Item::Const { name, .. } => Some(name),
            Item::Impl { .. } => None,
        }
    }
}

/// One `pub use` leaf, flattened out of its tree: `pub use a::{b, c as d,
/// e::*}` is three of these.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Use {
    /// The path segments, `crate`/`self`/`super` kept as written.
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub glob: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct ModTree {
    pub items: Vec<Item>,
    pub uses: Vec<Use>,
    /// Every child module, public or not: a private module's items reach
    /// the API through a `pub use`.
    pub mods: BTreeMap<String, ModTree>,
    pub pub_mods: BTreeSet<String>,
    /// What the walk could not read: a module file it did not find, a
    /// re-export from another crate.
    pub notes: Vec<String>,
    /// Item-position macro invocations and exported macro definitions seen.
    pub macro_items: usize,
}

/// Render a syntax node the way it was written, minus the spacing a token
/// stream prints between every token.
pub(crate) fn render<T: ToTokens>(node: &T) -> String {
    tidy_tokens(&node.to_token_stream().to_string())
}

/// Collapse a token stream's spacing back to source spacing: no space
/// before a closing bracket or a separator, none after an opening bracket,
/// a path separator or a reference sigil. Signatures print through here so
/// the index reads `fn open(name: &str) -> Vec<Dial>` and not the stream's
/// `fn open (name : & str) -> Vec < Dial >`.
pub(crate) fn tidy_tokens(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c != ' ' {
            out.push(c);
            continue;
        }
        let prev = if i > 0 { chars[i - 1] } else { ' ' };
        let next = chars.get(i + 1).copied().unwrap_or(' ');
        let after_path_sep = prev == ':' && i >= 2 && chars[i - 2] == ':';
        let before_path_sep = next == ':' && chars.get(i + 2) == Some(&':');
        let drop = matches!(
            next,
            ',' | ';' | ')' | ']' | '>' | ':' | '?' | '.' | '(' | '<'
        ) && !(next == '(' && prev == ',')
            && !(next == '<' && prev == ',')
            || matches!(prev, '(' | '[' | '<' | '&' | '!' | '.' | '*' | '\'')
            || after_path_sep
            || before_path_sep;
        if !drop {
            out.push(' ');
        }
    }
    out
}

fn doc_of(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .find_map(|a| match &a.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }),
                ..
            }) => {
                let line = s.value().trim().to_string();
                (!line.is_empty()).then_some(line)
            }
            _ => None,
        })
        .unwrap_or_default()
}

fn is_pub(vis: &syn::Visibility) -> bool {
    matches!(vis, syn::Visibility::Public(_))
}

fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().is_ident("cfg") && a.to_token_stream().to_string().contains("test"))
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("path"))
        .find_map(|a| match &a.meta {
            syn::Meta::NameValue(syn::MetaNameValue {
                value:
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }),
                ..
            }) => Some(s.value()),
            _ => None,
        })
}

fn method_of(sig: &syn::Signature) -> Method {
    Method {
        name: sig.ident.to_string(),
        sig: render(sig),
        is_static: sig.receiver().is_none(),
    }
}

/// The last segment of a type's path, generics dropped: what an impl block
/// is attached by.
fn type_head(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        syn::Type::Reference(r) => type_head(&r.elem),
        syn::Type::Group(g) => type_head(&g.elem),
        syn::Type::Paren(g) => type_head(&g.elem),
        _ => None,
    }
}

fn flatten_use(tree: &syn::UseTree, prefix: &[String], out: &mut Vec<Use>) {
    match tree {
        syn::UseTree::Path(p) => {
            let mut prefix = prefix.to_vec();
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, &prefix, out);
        }
        syn::UseTree::Name(n) => {
            let mut path = prefix.to_vec();
            path.push(n.ident.to_string());
            out.push(Use {
                path,
                alias: None,
                glob: false,
            });
        }
        syn::UseTree::Rename(r) => {
            let mut path = prefix.to_vec();
            path.push(r.ident.to_string());
            out.push(Use {
                path,
                alias: Some(r.rename.to_string()),
                glob: false,
            });
        }
        syn::UseTree::Glob(_) => out.push(Use {
            path: prefix.to_vec(),
            alias: None,
            glob: true,
        }),
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, out);
            }
        }
    }
}

/// Where a file's `mod x;` children live: beside `lib.rs` and `mod.rs`,
/// under `<stem>/` for any other file.
fn child_dir(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("lib") | Some("main") | Some("mod") => dir,
        Some(stem) => dir.join(stem),
        None => dir,
    }
}

struct Walk {
    visited: BTreeSet<PathBuf>,
}

const MAX_DEPTH: usize = 64;

impl Walk {
    fn file(&mut self, path: &Path, depth: usize) -> ModTree {
        let mut tree = ModTree::default();
        let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if depth > MAX_DEPTH || !self.visited.insert(canonical) {
            tree.notes
                .push(format!("module {} was not read (a cycle)", path.display()));
            return tree;
        }
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                tree.notes
                    .push(format!("module {} was not read ({e})", path.display()));
                return tree;
            }
        };
        match syn::parse_file(&text) {
            Ok(file) => self.items(&file.items, path, &child_dir(path), depth, &mut tree),
            Err(e) => tree
                .notes
                .push(format!("module {} did not parse ({e})", path.display())),
        }
        tree
    }

    fn items(
        &mut self,
        items: &[syn::Item],
        file: &Path,
        children: &Path,
        depth: usize,
        tree: &mut ModTree,
    ) {
        for item in items {
            match item {
                syn::Item::Fn(f) if is_pub(&f.vis) && !is_cfg_test(&f.attrs) => {
                    tree.items.push(Item::Fn {
                        name: f.sig.ident.to_string(),
                        sig: render(&f.sig),
                        doc: doc_of(&f.attrs),
                    })
                }
                syn::Item::Struct(s) if is_pub(&s.vis) && !is_cfg_test(&s.attrs) => {
                    let fields = match &s.fields {
                        syn::Fields::Named(named) => named
                            .named
                            .iter()
                            .filter(|f| is_pub(&f.vis))
                            .map(|f| {
                                (
                                    f.ident
                                        .as_ref()
                                        .map(ToString::to_string)
                                        .unwrap_or_default(),
                                    render(&f.ty),
                                )
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    tree.items.push(Item::Struct {
                        name: s.ident.to_string(),
                        generics: render(&s.generics),
                        fields,
                        doc: doc_of(&s.attrs),
                    })
                }
                syn::Item::Enum(e) if is_pub(&e.vis) && !is_cfg_test(&e.attrs) => {
                    let variants = e
                        .variants
                        .iter()
                        .map(|v| {
                            let payload = match &v.fields {
                                syn::Fields::Unit => String::new(),
                                fields => render(fields),
                            };
                            (v.ident.to_string(), payload)
                        })
                        .collect();
                    tree.items.push(Item::Enum {
                        name: e.ident.to_string(),
                        generics: render(&e.generics),
                        variants,
                        doc: doc_of(&e.attrs),
                    })
                }
                syn::Item::Trait(t) if is_pub(&t.vis) && !is_cfg_test(&t.attrs) => {
                    let methods = t
                        .items
                        .iter()
                        .filter_map(|i| match i {
                            syn::TraitItem::Fn(f) => Some(method_of(&f.sig)),
                            _ => None,
                        })
                        .collect();
                    tree.items.push(Item::Trait {
                        name: t.ident.to_string(),
                        methods,
                        doc: doc_of(&t.attrs),
                    })
                }
                syn::Item::Type(t) if is_pub(&t.vis) && !is_cfg_test(&t.attrs) => {
                    tree.items.push(Item::TypeAlias {
                        name: t.ident.to_string(),
                        target: render(&t.ty),
                        doc: doc_of(&t.attrs),
                    })
                }
                syn::Item::Const(c) if is_pub(&c.vis) && !is_cfg_test(&c.attrs) => {
                    tree.items.push(Item::Const {
                        name: c.ident.to_string(),
                        ty: render(&c.ty),
                        doc: doc_of(&c.attrs),
                    })
                }
                syn::Item::Static(c) if is_pub(&c.vis) && !is_cfg_test(&c.attrs) => {
                    tree.items.push(Item::Const {
                        name: c.ident.to_string(),
                        ty: render(&c.ty),
                        doc: doc_of(&c.attrs),
                    })
                }
                syn::Item::Impl(i) if !is_cfg_test(&i.attrs) => {
                    let Some(self_ty) = type_head(&i.self_ty) else {
                        continue;
                    };
                    let trait_ = i
                        .trait_
                        .as_ref()
                        .and_then(|(_, p, _)| p.segments.last())
                        .map(|s| s.ident.to_string());
                    // An inherent method needs `pub`; a trait's methods are
                    // as public as the trait.
                    let methods = i
                        .items
                        .iter()
                        .filter_map(|it| match it {
                            syn::ImplItem::Fn(f) if trait_.is_some() || is_pub(&f.vis) => {
                                Some(method_of(&f.sig))
                            }
                            _ => None,
                        })
                        .collect();
                    tree.items.push(Item::Impl {
                        self_ty,
                        trait_,
                        methods,
                    })
                }
                syn::Item::Use(u) if is_pub(&u.vis) && !is_cfg_test(&u.attrs) => {
                    let mut leaves = Vec::new();
                    flatten_use(&u.tree, &[], &mut leaves);
                    tree.uses.extend(leaves);
                }
                syn::Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                    let name = m.ident.to_string();
                    let child = match &m.content {
                        Some((_, items)) => {
                            let mut child = ModTree::default();
                            self.items(items, file, &children.join(&name), depth + 1, &mut child);
                            child
                        }
                        None => {
                            let candidates = match path_attr(&m.attrs) {
                                Some(p) => vec![file.parent().unwrap_or(Path::new(".")).join(p)],
                                None => vec![
                                    children.join(format!("{name}.rs")),
                                    children.join(&name).join("mod.rs"),
                                ],
                            };
                            match candidates.iter().find(|p| p.is_file()) {
                                Some(p) => self.file(p, depth + 1),
                                None => {
                                    let mut child = ModTree::default();
                                    child.notes.push(format!(
                                        "module {name} was not found beside {}",
                                        file.display()
                                    ));
                                    child
                                }
                            }
                        }
                    };
                    if is_pub(&m.vis) {
                        tree.pub_mods.insert(name.clone());
                    }
                    tree.mods.insert(name, child);
                }
                syn::Item::Macro(_) => tree.macro_items += 1,
                _ => {}
            }
        }
    }
}

/// The crate rooted at `lib_rs`, every `mod x;` followed to its file.
pub(crate) fn load_crate(lib_rs: &Path) -> Result<ModTree, String> {
    if !lib_rs.is_file() {
        return Err(format!("{} is not a file", lib_rs.display()));
    }
    let mut walk = Walk {
        visited: BTreeSet::new(),
    };
    Ok(walk.file(lib_rs, 0))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    /// A stand-in crate exercising what the walk must follow: a public
    /// module in its own file, a private module re-exported, a `mod.rs`
    /// module with a glob re-export, a `#[path]` module, an inline module,
    /// impl blocks, a local trait, an enum, a macro and test-only items.
    pub(crate) fn fixture_crate(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("tono-index-rust-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        write(
            &src.join("lib.rs"),
            "//! The gearbox crate.\n\
             pub mod a;\n\
             mod b;\n\
             pub mod c;\n\
             #[path = \"elsewhere.rs\"]\n\
             pub mod d;\n\
             pub mod e { pub fn inline_fn() {} pub(crate) fn hidden() {} }\n\
             pub use b::Hidden;\n\
             pub use self::a::Dial as Meter;\n\
             pub use other_crate::Foreign;\n\
             /// Open a dial.\n\
             pub fn open(name: &str, opts: Option<&Options>) -> Result<Dial, Error> { todo!() }\n\
             pub(crate) fn nope() {}\n\
             pub struct Options { pub precision: u8, secret: u8 }\n\
             pub struct Error;\n\
             impl Options { pub fn new() -> Self { todo!() } pub fn get(&self) -> u8 { 0 } fn private(&self) {} }\n\
             pub trait Run { fn run(&self) -> u8; fn make() -> Self; }\n\
             impl Run for Options { fn run(&self) -> u8 { 0 } fn make() -> Self { todo!() } }\n\
             pub enum Mode { Fast, Slow(u8), Custom { level: u8 } }\n\
             pub type Pair<T> = (T, T);\n\
             pub const LIMIT: usize = 3;\n\
             #[macro_export]\n\
             macro_rules! shout { () => {} }\n\
             #[cfg(test)]\n\
             pub fn only_in_tests() {}\n\
             #[cfg(test)]\n\
             mod tests { pub fn t() {} }\n",
        );
        write(
            &src.join("a.rs"),
            "/// A dial.\npub struct Dial<T> { pub value: T }\nimpl<T: Copy> Dial<T> { pub fn read(&self) -> T { self.value } }\npub mod deep;\n",
        );
        write(&src.join("a/deep.rs"), "pub fn deepest() {}\n");
        write(
            &src.join("b.rs"),
            "pub struct Hidden;\npub fn unreachable_fn() {}\n",
        );
        write(
            &src.join("c/mod.rs"),
            "pub use super::a::*;\npub use crate::b;\npub fn c_fn() {}\n",
        );
        write(
            &src.join("elsewhere.rs"),
            "pub fn moved() {}\nmod missing;\n",
        );
        dir
    }

    #[test]
    fn tidy_tokens_restores_source_spacing() {
        for (stream, tidy) in [
            (
                "fn open (name : & str) -> Vec < Dial >",
                "fn open(name: &str) -> Vec<Dial>",
            ),
            ("Option < & 'a str >", "Option<&'a str>"),
            ("std :: fmt :: Result", "std::fmt::Result"),
            ("fn get (& self) -> u8", "fn get(&self) -> u8"),
            (
                "fn f < T : Clone > (x : T) where T : Copy",
                "fn f<T: Clone>(x: T) where T: Copy",
            ),
            ("Vec < Vec < T >>", "Vec<Vec<T>>"),
            ("* const u8", "*const u8"),
            ("fn g (a : u8 , b : u8) -> ! ", "fn g(a: u8, b: u8) -> !"),
            ("Box < dyn Fn (u8) -> u8 >", "Box<dyn Fn(u8) -> u8>"),
            ("[u8 ; 4]", "[u8; 4]"),
        ] {
            assert_eq!(tidy_tokens(stream), tidy, "{stream}");
        }
    }

    #[test]
    fn the_walk_collects_public_items_per_module() {
        let dir = fixture_crate("walk");
        let tree = load_crate(&dir.join("src/lib.rs")).unwrap();
        let names: Vec<&str> = tree.items.iter().filter_map(Item::name).collect();
        assert_eq!(
            names,
            vec!["open", "Options", "Error", "Run", "Mode", "Pair", "LIMIT"]
        );
        let open = &tree.items[0];
        assert_eq!(
            open,
            &Item::Fn {
                name: "open".into(),
                sig: "fn open(name: &str, opts: Option<&Options>) -> Result<Dial, Error>".into(),
                doc: "Open a dial.".into(),
            }
        );
        match &tree.items[1] {
            Item::Struct { fields, .. } => {
                assert_eq!(fields, &vec![("precision".to_string(), "u8".to_string())])
            }
            other => panic!("{other:?}"),
        }
        type ImplShape<'a> = (&'a str, Option<&'a str>, Vec<(&'a str, bool)>);
        let impls: Vec<ImplShape> = tree
            .items
            .iter()
            .filter_map(|i| match i {
                Item::Impl {
                    self_ty,
                    trait_,
                    methods,
                } => Some((
                    self_ty.as_str(),
                    trait_.as_deref(),
                    methods
                        .iter()
                        .map(|m| (m.name.as_str(), m.is_static))
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            impls,
            vec![
                ("Options", None, vec![("new", true), ("get", false)]),
                ("Options", Some("Run"), vec![("run", false), ("make", true)]),
            ]
        );
        let mode = tree
            .items
            .iter()
            .find(|i| i.name() == Some("Mode"))
            .unwrap();
        match mode {
            Item::Enum { variants, .. } => assert_eq!(
                variants,
                &vec![
                    ("Fast".to_string(), String::new()),
                    ("Slow".to_string(), "(u8)".to_string()),
                    ("Custom".to_string(), "{ level: u8 }".to_string()),
                ]
            ),
            other => panic!("{other:?}"),
        }
        assert_eq!(tree.macro_items, 1);
        assert_eq!(
            tree.uses,
            vec![
                Use {
                    path: vec!["b".into(), "Hidden".into()],
                    alias: None,
                    glob: false
                },
                Use {
                    path: vec!["self".into(), "a".into(), "Dial".into()],
                    alias: Some("Meter".into()),
                    glob: false
                },
                Use {
                    path: vec!["other_crate".into(), "Foreign".into()],
                    alias: None,
                    glob: false
                },
            ]
        );
    }

    #[test]
    fn the_walk_follows_modules_to_their_files() {
        let dir = fixture_crate("mods");
        let tree = load_crate(&dir.join("src/lib.rs")).unwrap();
        let mods: Vec<&String> = tree.mods.keys().collect();
        assert_eq!(mods, vec!["a", "b", "c", "d", "e"]);
        assert_eq!(
            tree.pub_mods.iter().collect::<Vec<_>>(),
            vec!["a", "c", "d", "e"]
        );
        let a = &tree.mods["a"];
        assert_eq!(a.items[0].name(), Some("Dial"));
        assert_eq!(a.mods["deep"].items[0].name(), Some("deepest"));
        assert_eq!(tree.mods["b"].items[0].name(), Some("Hidden"));
        let c = &tree.mods["c"];
        assert!(c.uses[0].glob);
        assert_eq!(c.uses[0].path, vec!["super", "a"]);
        assert_eq!(c.items[0].name(), Some("c_fn"));
        let d = &tree.mods["d"];
        assert_eq!(d.items[0].name(), Some("moved"));
        assert!(
            d.mods["missing"].notes[0].contains("was not found"),
            "{:?}",
            d.notes
        );
        assert_eq!(tree.mods["e"].items.len(), 1);
        assert!(!tree.mods.contains_key("tests"));
    }

    #[test]
    fn a_missing_root_is_an_error_and_a_cycle_is_a_note() {
        assert!(load_crate(Path::new("/nonexistent/lib.rs")).is_err());
        let dir =
            std::env::temp_dir().join(format!("tono-index-rust-{}-cycle", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write(
            &dir.join("src/lib.rs"),
            "#[path = \"lib.rs\"]\npub mod again;\n",
        );
        let tree = load_crate(&dir.join("src/lib.rs")).unwrap();
        assert!(tree.mods["again"].notes[0].contains("a cycle"));
        write(&dir.join("src/bad.rs"), "pub fn (");
        write(&dir.join("src/lib.rs"), "pub mod bad;\n");
        let tree = load_crate(&dir.join("src/lib.rs")).unwrap();
        assert!(tree.mods["bad"].notes[0].contains("did not parse"));
    }
}

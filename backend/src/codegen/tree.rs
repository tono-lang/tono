//! The shared, non-string component tree.
//!
//! Every node references `Symbol`s (never raw type names), so a file's imports
//! are derived by folding the symbols reachable from its declarations rather
//! than written by hand. The tree is target-agnostic: each language backend
//! supplies a Symbol table plus render rules, but the node set here is shared.

use crate::codegen::group::Group;
use crate::codegen::symbol::Symbol;

/// A source file: a module name plus its top-level declarations. Imports are
/// DERIVED (see the `imports` module) by folding the symbols reachable from the
/// declarations; an import whose module equals `module` is dropped, since a type
/// defined in this file needs no import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub module: String,
    pub decls: Vec<Decl>,
}

/// One emission group's output file: the group it belongs to (which decides
/// where it lands and how visible it is) plus the file itself.
///
/// `provides` names the symbols the file declares through opaque text (a Raw
/// item: an `impl` block, a helper module, a macro), which the index that maps a
/// symbol to its defining group cannot read off the tree. Everything declared
/// structurally is picked up without being listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFile {
    pub group: Group,
    pub file: File,
    pub provides: Vec<String>,
}

impl ModuleFile {
    /// A group's file with nothing declared through opaque text. The file's
    /// module is the group's path, since a group is what a module path denotes
    /// once a module spans several files.
    pub fn new(group: Group, decls: Vec<Decl>) -> Self {
        Self {
            file: File {
                module: group.path(),
                decls,
            },
            group,
            provides: Vec::new(),
        }
    }

    /// Declare the symbols this file's raw items define, so references to them
    /// from other groups resolve to this file.
    #[must_use]
    pub fn providing(mut self, provides: Vec<String>) -> Self {
        self.provides = provides;
        self
    }
}

/// A top-level declaration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decl {
    Interface(Interface),
    Enum(EnumDecl),
    Union(UnionDecl),
    Method(Method),
    Function(Function),
    Alias(Alias),
    Raw(Raw),
    Client(ClientDecl),
}

/// A named type alias whose definition is target text (e.g. a branded
/// well-known type). The definition references no symbols the engine must
/// import, so it carries none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alias {
    pub name: Symbol,
    pub value: String,
}

/// A placeholder for a reference inside opaque text, replaced when the file is
/// rendered by the target's spelling of that symbol.
///
/// Opaque text is the one thing the engine cannot re-point the way it re-points
/// the tree, so a name that may turn out to live in another compilation unit
/// (Go writes a package selector for one) has to be written as a slot rather
/// than spelled where it is emitted. The slot names the symbol; the emitter
/// carries the matching `Symbol` in the item's `refs`, which is also what makes
/// the import collected.
///
/// The delimiter is a control character, so it cannot occur in source a spec
/// could produce.
pub fn symbol_slot(name: &str) -> String {
    format!("\u{1}{name}\u{1}")
}

/// Replace every [`symbol_slot`] in `text` with `spell(symbol)` for the matching
/// reference. A slot with no matching reference keeps the bare name: the import
/// would be missing either way, and a control character in the output would turn
/// a missing declaration into an unreadable one.
pub fn fill_symbol_slots(text: &str, refs: &[Symbol], spell: &dyn Fn(&Symbol) -> String) -> String {
    if !text.contains('\u{1}') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut parts = text.split('\u{1}');
    if let Some(first) = parts.next() {
        out.push_str(first);
    }
    let mut in_slot = true;
    for part in parts {
        if in_slot {
            match refs.iter().find(|s| s.name == part) {
                Some(symbol) => out.push_str(&spell(symbol)),
                None => out.push_str(part),
            }
        } else {
            out.push_str(part);
        }
        in_slot = !in_slot;
    }
    out
}

/// The references an item declares, which is what a slot in its text resolves
/// against. Only the items carrying opaque text have any.
pub fn item_refs(decl: &Decl) -> &[Symbol] {
    match decl {
        Decl::Raw(raw) => &raw.refs,
        Decl::Function(function) => {
            let FnBody::Raw { refs, .. } = &function.body;
            refs
        }
        _ => &[],
    }
}

/// A fully-formed top-level item rendered verbatim, for constructs the shared
/// node set does not model (an `impl` block, a helper module, a method with a
/// receiver). Its `refs` declare the symbols the text references so import
/// collection still reaches them, exactly like a function body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Raw {
    pub text: String,
    pub refs: Vec<Symbol>,
    /// The names the text declares. Opaque text cannot be read for them, so an
    /// item that something else may reference by name says so here.
    pub provides: Vec<String>,
}

impl Decl {
    /// A verbatim item referencing no symbols the engine must import.
    pub fn raw(text: impl Into<String>) -> Decl {
        Decl::Raw(Raw {
            text: text.into(),
            ..Raw::default()
        })
    }

    /// A verbatim item declaring the symbols its text references, so import
    /// collection still reaches them.
    pub fn raw_with(text: impl Into<String>, refs: Vec<Symbol>) -> Decl {
        Decl::Raw(Raw {
            text: text.into(),
            refs,
            ..Raw::default()
        })
    }

    /// A verbatim item that names what it declares, so a reference from another
    /// group resolves to it and an unreferenced one can be left out.
    pub fn raw_providing(name: &str, text: impl Into<String>, refs: Vec<Symbol>) -> Decl {
        Decl::Raw(Raw {
            text: text.into(),
            refs,
            provides: vec![name.to_string()],
        })
    }

    /// The verbatim source of an item that carries opaque text, or `None` for one
    /// the tree models structurally. What a reader has to scan when the tree
    /// cannot answer which names an item reaches.
    pub fn opaque_text(&self) -> Option<&str> {
        match self {
            Decl::Raw(raw) => Some(&raw.text),
            Decl::Function(f) => {
                let FnBody::Raw { text, .. } = &f.body;
                Some(text)
            }
            _ => None,
        }
    }
}

/// A product type: a named structure/interface with fields. `params` are the
/// generic type parameters of the definition (already cased to the target's type
/// spelling), empty for a non-generic shape; they render as the declaration's type
/// parameter clause (`<T>` / `[T any]`) while a reference that applies them rides
/// `TypeExpr::Generic`. `deprecated` carries the `@deprecated` reason (`Some("")`
/// when marked without one), rendered as each target's native deprecation
/// annotation. `doc` carries the `@doc` Markdown, lowered to each target's native
/// documentation comment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: Symbol,
    pub params: Vec<String>,
    pub fields: Vec<Field>,
    pub deprecated: Option<String>,
    pub doc: Option<String>,
}

/// A field: an identifier symbol, its type expression, nullability, and an
/// optional wire-key override. The `wire` override and the identifier are
/// independent axes: `wire` feeds the target's serialization-rename mechanism
/// and never changes the in-code name; the name carries the casing / `@rename`
/// result and never changes the wire key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: Symbol,
    pub ty: TypeExpr,
    pub nullable: bool,
    pub wire: Option<String>,
    /// The `@deprecated` reason (`Some("")` when marked without one). Rendered as
    /// the target's native field deprecation. Always `None` for a method/function
    /// parameter, which is never deprecated.
    pub deprecated: Option<String>,
    /// The `@doc` Markdown for this field, or `None`. Rendered as the target's
    /// native member documentation. `None` for a method/function parameter.
    pub doc: Option<String>,
}

/// An operation stub: a typed signature. The opaque wire descriptor and the
/// `runtime.execute` call are emitted by the target; this node carries only the
/// shape the engine needs to collect imports and render the signature. The
/// effect (`is_async`) is classified once in the shared model; how the wait
/// lowers is a render concern (a `Promise`/`async fn` or a blocking call). The
/// error channel (`err`) likewise renders per idiom: a `Result` error type, a
/// `(T, error)` pair, or nothing where errors are thrown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Method {
    pub name: Symbol,
    pub params: Vec<Field>,
    pub ret: Option<TypeExpr>,
    pub err: Option<TypeExpr>,
    pub is_async: bool,
    /// The `@doc` Markdown for the operation, or `None`. Rendered as the target's
    /// native method documentation.
    pub doc: Option<String>,
}

/// The generated client surface of a module: one method signature per
/// operation. Emitted as an interface/trait so the transport implementation
/// (hand-written or a later runtime) can satisfy it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientDecl {
    pub name: Symbol,
    pub methods: Vec<Method>,
}

/// A free function with a real body, used for generated codecs and helpers. Its
/// signature is symbol-typed so import collection sees its parameter and return
/// types; the body additionally declares the symbols it references so those
/// imports are collected too, even though the body statements are target text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Function {
    pub name: Symbol,
    pub params: Vec<Field>,
    pub ret: Option<TypeExpr>,
    pub body: FnBody,
}

/// A function body. The statements are rendered text (the formatter is the
/// layout authority), paired with the symbols the text references so the engine
/// can still collect their imports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FnBody {
    Raw { text: String, refs: Vec<Symbol> },
}

/// How an enum's members travel on the wire. A string-backed enum sends each
/// member as its verbatim wire name; an int-backed enum sends an explicit integer
/// per member, carried here parallel to the decl's `members`. Whether the enum is
/// open (its lenient `Unknown` arm) is a target render concern, not stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumRepr {
    String,
    Int(Vec<i64>),
}

/// An enumeration: a name, its members (each an idiomatic-cased symbol), and how
/// they are backed on the wire. The open-enum `Unknown` arm is a target render
/// concern, not stored here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnumDecl {
    pub name: Symbol,
    pub members: Vec<Symbol>,
    /// Per-member `@doc`, parallel to `members` (same length): the doc for
    /// `members[i]` is `member_docs[i]`, or `None`. Kept parallel like the wire
    /// integers in `EnumRepr::Int`, since a member is a bare symbol here.
    pub member_docs: Vec<Option<String>>,
    pub backing: EnumRepr,
    pub deprecated: Option<String>,
    pub doc: Option<String>,
}

/// An internally-tagged union: a discriminator field name (default `type`) and
/// its variants. Each variant is a struct so the discriminator field can live in
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionDecl {
    pub name: Symbol,
    pub discriminator: String,
    pub variants: Vec<Variant>,
    pub deprecated: Option<String>,
    pub doc: Option<String>,
}

/// A union variant. Its `name` is the wire tag (overridable by `wire`). A
/// variant carries its payload either inline as `fields` or, when the IR
/// references a payload shape, as a `payload` type the discriminator object is
/// intersected with; the two are alternatives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub name: Symbol,
    pub fields: Vec<Field>,
    pub payload: Option<TypeExpr>,
    pub wire: Option<String>,
    /// The `@doc` Markdown for this variant, or `None`. Rendered as the target's
    /// native documentation on the variant type.
    pub doc: Option<String>,
}

/// A composable type expression over Symbols.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TypeExpr {
    Ref(Symbol),
    List(Box<TypeExpr>),
    Map(Box<TypeExpr>, Box<TypeExpr>),
    Nullable(Box<TypeExpr>),
    Generic(Symbol, Vec<TypeExpr>),
    /// An `@entries` map: an ordered sequence of `(key, value)` pairs that travels
    /// on the wire as an array of two-element arrays `[[k, v], …]` instead of a
    /// JSON object, the escape for a non-string-coercible map key. The in-code
    /// shape is target-specific (a tuple list / pair list), but the wire shape is
    /// shared, so a target renders this rather than `Map`.
    Entries(Box<TypeExpr>, Box<TypeExpr>),
}

impl TypeExpr {
    /// `List<inner>`.
    pub fn list(inner: TypeExpr) -> Self {
        TypeExpr::List(Box::new(inner))
    }

    /// `Map<key, value>`.
    pub fn map(key: TypeExpr, value: TypeExpr) -> Self {
        TypeExpr::Map(Box::new(key), Box::new(value))
    }

    /// `inner?` (nullable).
    pub fn nullable(inner: TypeExpr) -> Self {
        TypeExpr::Nullable(Box::new(inner))
    }

    /// An `@entries` pairs-array of `(key, value)`.
    pub fn entries(key: TypeExpr, value: TypeExpr) -> Self {
        TypeExpr::Entries(Box::new(key), Box::new(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page_of_charge() -> TypeExpr {
        // A generic application Page<Charge>, the cross-module transitive case.
        TypeExpr::Generic(
            Symbol::imported("Page", "core", "Page"),
            vec![TypeExpr::Ref(Symbol::imported(
                "Charge", "payments", "Charge",
            ))],
        )
    }

    #[test]
    fn type_expr_constructors_box_their_children() {
        assert_eq!(
            TypeExpr::list(TypeExpr::Ref(Symbol::builtin("string"))),
            TypeExpr::List(Box::new(TypeExpr::Ref(Symbol::builtin("string"))))
        );
        assert_eq!(
            TypeExpr::map(
                TypeExpr::Ref(Symbol::builtin("string")),
                TypeExpr::Ref(Symbol::builtin("i64")),
            ),
            TypeExpr::Map(
                Box::new(TypeExpr::Ref(Symbol::builtin("string"))),
                Box::new(TypeExpr::Ref(Symbol::builtin("i64"))),
            )
        );
        assert_eq!(
            TypeExpr::nullable(TypeExpr::Ref(Symbol::builtin("bool"))),
            TypeExpr::Nullable(Box::new(TypeExpr::Ref(Symbol::builtin("bool"))))
        );
    }

    #[test]
    fn a_file_composes_every_declaration_kind() {
        let file = File {
            module: "payments".into(),
            decls: vec![
                Decl::Interface(Interface {
                    name: Symbol::builtin("Charge"),
                    params: vec![],
                    fields: vec![
                        Field {
                            name: Symbol::builtin("id"),
                            ty: TypeExpr::Ref(Symbol::builtin("string")),
                            nullable: false,
                            wire: None,
                            deprecated: None,
                            doc: None,
                        },
                        Field {
                            name: Symbol::builtin("page"),
                            ty: page_of_charge(),
                            nullable: true,
                            wire: Some("page_ref".into()),
                            deprecated: None,
                            doc: None,
                        },
                    ],
                    deprecated: None,
                    doc: None,
                }),
                Decl::Enum(EnumDecl {
                    name: Symbol::builtin("Status"),
                    members: vec![Symbol::builtin("Active"), Symbol::builtin("Closed")],
                    member_docs: vec![None, None],
                    backing: EnumRepr::String,
                    deprecated: None,
                    doc: None,
                }),
                Decl::Union(UnionDecl {
                    name: Symbol::builtin("Method"),
                    discriminator: "type".into(),
                    variants: vec![Variant {
                        name: Symbol::builtin("Card"),
                        fields: vec![],
                        payload: None,
                        wire: Some("card".into()),
                        doc: None,
                    }],
                    deprecated: None,
                    doc: None,
                }),
                Decl::Method(Method {
                    name: Symbol::builtin("create_charge"),
                    params: vec![Field {
                        name: Symbol::builtin("input"),
                        ty: TypeExpr::Ref(Symbol::imported("Charge", "payments", "Charge")),
                        nullable: false,
                        wire: None,
                        deprecated: None,
                        doc: None,
                    }],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                    err: None,
                    is_async: false,
                    doc: None,
                }),
                Decl::Client(ClientDecl {
                    name: Symbol::builtin("Client"),
                    methods: vec![Method {
                        name: Symbol::builtin("create_charge"),
                        params: vec![],
                        ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                        err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                        is_async: true,
                        doc: None,
                    }],
                }),
            ],
        };
        assert_eq!(file.module, "payments");
        assert_eq!(file.decls.len(), 5);
    }
}

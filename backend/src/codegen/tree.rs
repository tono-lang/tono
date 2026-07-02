//! The shared, non-string component tree.
//!
//! Every node references `Symbol`s (never raw type names), so a file's imports
//! are derived by folding the symbols reachable from its declarations rather
//! than written by hand. The tree is target-agnostic: each language backend
//! supplies a Symbol table plus render rules, but the node set here is shared.

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

/// One output file for a module: a basename suffix ("" for the main types file,
/// "_serde" for the serialization file) plus the file itself. A module can emit
/// more than one so types and serialization land in separate files.
///
/// In Go the split files share one package, so a self-module symbol still needs no
/// import (`imports_companion` is `None`). In TypeScript and Rust the split files
/// are separate modules, so the serde file references types that live in the types
/// file: when `imports_companion` names the companion module path, the import
/// engine redirects self-module symbols to an import of that companion instead of
/// dropping them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleFile {
    pub suffix: &'static str,
    pub file: File,
    pub imports_companion: Option<String>,
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

/// A fully-formed top-level item rendered verbatim, for constructs the shared
/// node set does not model (an `impl` block, a helper module, a method with a
/// receiver). Its `refs` declare the symbols the text references so import
/// collection still reaches them, exactly like a function body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw {
    pub text: String,
    pub refs: Vec<Symbol>,
}

impl Decl {
    /// A verbatim item referencing no symbols the engine must import.
    pub fn raw(text: impl Into<String>) -> Decl {
        Decl::Raw(Raw {
            text: text.into(),
            refs: Vec::new(),
        })
    }

    /// A verbatim item declaring the symbols its text references, so import
    /// collection still reaches them.
    pub fn raw_with(text: impl Into<String>, refs: Vec<Symbol>) -> Decl {
        Decl::Raw(Raw {
            text: text.into(),
            refs,
        })
    }
}

/// A product type: a named structure/interface with fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Interface {
    pub name: Symbol,
    pub fields: Vec<Field>,
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
    pub backing: EnumRepr,
}

/// An internally-tagged union: a discriminator field name (default `type`) and
/// its variants. Each variant is a struct so the discriminator field can live in
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnionDecl {
    pub name: Symbol,
    pub discriminator: String,
    pub variants: Vec<Variant>,
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
                    fields: vec![
                        Field {
                            name: Symbol::builtin("id"),
                            ty: TypeExpr::Ref(Symbol::builtin("string")),
                            nullable: false,
                            wire: None,
                        },
                        Field {
                            name: Symbol::builtin("page"),
                            ty: page_of_charge(),
                            nullable: true,
                            wire: Some("page_ref".into()),
                        },
                    ],
                }),
                Decl::Enum(EnumDecl {
                    name: Symbol::builtin("Status"),
                    members: vec![Symbol::builtin("Active"), Symbol::builtin("Closed")],
                    backing: EnumRepr::String,
                }),
                Decl::Union(UnionDecl {
                    name: Symbol::builtin("Method"),
                    discriminator: "type".into(),
                    variants: vec![Variant {
                        name: Symbol::builtin("Card"),
                        fields: vec![],
                        payload: None,
                        wire: Some("card".into()),
                    }],
                }),
                Decl::Method(Method {
                    name: Symbol::builtin("create_charge"),
                    params: vec![Field {
                        name: Symbol::builtin("input"),
                        ty: TypeExpr::Ref(Symbol::imported("Charge", "payments", "Charge")),
                        nullable: false,
                        wire: None,
                    }],
                    ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                    err: None,
                    is_async: false,
                }),
                Decl::Client(ClientDecl {
                    name: Symbol::builtin("Client"),
                    methods: vec![Method {
                        name: Symbol::builtin("create_charge"),
                        params: vec![],
                        ret: Some(TypeExpr::Ref(Symbol::builtin("Charge"))),
                        err: Some(TypeExpr::Ref(Symbol::builtin("TonoError"))),
                        is_async: true,
                    }],
                }),
            ],
        };
        assert_eq!(file.module, "payments");
        assert_eq!(file.decls.len(), 5);
    }
}

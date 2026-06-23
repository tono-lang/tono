//! The typed codegen engine: an alloy-style core that turns the IR into
//! idiomatic per-language source code.
//!
//! The engine manipulates a tree of typed components holding `Symbol`s (never
//! raw strings), collects imports automatically from referenced symbols, and
//! runs each language's official formatter as the single layout authority. This
//! module owns the language-agnostic core; per-language Symbol tables and render
//! rules are supplied by target backends.

pub mod symbol;
pub mod tree;

pub use symbol::{Import, Symbol, SymbolKind};
pub use tree::{Decl, EnumDecl, Field, File, Interface, Method, TypeExpr, UnionDecl, Variant};

//! The typed codegen engine: an alloy-style core that turns the IR into
//! idiomatic per-language source code.
//!
//! The engine manipulates a tree of typed components holding `Symbol`s (never
//! raw strings), collects imports automatically from referenced symbols, and
//! runs each language's official formatter as the single layout authority. This
//! module owns the language-agnostic core; per-language Symbol tables and render
//! rules are supplied by target backends.

pub mod casing;
pub mod conventions;
pub mod format;
pub mod imports;
pub mod ops;
pub mod pipeline;
pub mod render;
pub mod symbol;
pub mod syntax;
pub mod target;
pub mod targets;
#[cfg(test)]
pub mod test_support;
pub mod tree;

pub use casing::{CaseStyle, CasingConfig};
pub use format::{Formatted, Formatter};
pub use pipeline::{generate, parse_targets, GeneratedFile, TargetKind};
pub use render::render_file;
pub use symbol::{Import, Symbol, SymbolKind};
pub use target::{Fragment, RenderRules, Target};
pub use tree::{
    Alias, Decl, EnumDecl, Field, File, FnBody, Function, Interface, Method, Raw, TypeExpr,
    UnionDecl, Variant,
};

//! The typed codegen engine: an alloy-style core that turns the IR into
//! idiomatic per-language source code.
//!
//! The engine manipulates a tree of typed components holding `Symbol`s (never
//! raw strings), collects imports automatically from referenced symbols, and
//! runs each language's official formatter as the single layout authority. This
//! module owns the language-agnostic core; per-language Symbol tables and render
//! rules are supplied by target backends.

pub mod assemble;
pub mod casing;
pub mod check;
pub mod conventions;
pub mod declared_tests;
pub mod doc;
pub mod entries;
pub mod extensions;
pub mod fixtures;
pub mod format;
pub mod group;
pub mod imports;
pub mod layout;
pub mod modules;
pub mod ops;
pub mod output;
pub mod pipeline;
pub mod reexport;
pub mod render;
pub mod symbol;
pub mod syntax;
pub mod target;
pub mod targets;
pub mod taxonomy;
#[cfg(test)]
pub mod test_support;
mod traits;
pub mod tree;
pub mod validation;
pub mod visibility;

pub use assemble::resolve_groups;
pub use casing::{CaseStyle, CasingConfig};
pub use check::{check, CheckOutcome};
pub use format::{Formatted, Formatter, Warning};
pub use group::{Audience, Group, Origin};
pub use layout::check_layout;
pub use modules::CodegenConfig;
pub use output::{GeneratedFile, TargetKind};
pub use pipeline::{casing_for, generate, generate_target, is_generated, parse_targets};
pub use render::render_file;
pub use symbol::{Import, Symbol, SymbolKind};
pub use target::{Fragment, RenderRules, Target};
pub use tree::{
    Alias, Decl, EnumDecl, Field, File, FnBody, Function, Interface, Method, Raw, TypeExpr,
    UnionDecl, Variant,
};

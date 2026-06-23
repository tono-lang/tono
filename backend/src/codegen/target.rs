//! The target seam.
//!
//! A language backend supplies two things (per RFC-0005 / RFC-0006): a *Symbol
//! table* — how IR types map to language symbols and declarations — and *render
//! rules* — how the shared component tree turns into the language's surface
//! syntax. The [`Target`] trait is the Symbol table; [`RenderRules`] is the
//! render rules. The engine owns everything in between (import collection,
//! ordering, casing, the formatter), so a backend only adds these two pieces.

use serde_json::Value;

use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{Shape, Tref};

/// What a target emits for a shape: declaration nodes, never text. Emitting tree
/// nodes (rather than strings) is what lets the engine collect imports from the
/// symbols they reference before anything is rendered.
pub type Fragment = Vec<Decl>;

/// The per-language Symbol table and emitters. A target never interprets the
/// opaque wire descriptor — it embeds it as a blob and emits a `runtime.execute`
/// call — which keeps it blind to protocol (Protocol x Target orthogonality).
/// Casing and the formatter are engine utilities, not methods here.
pub trait Target {
    /// The target language identifier, e.g. `"typescript"`.
    fn name(&self) -> &str;

    /// Map an IR type reference to this language's symbol for it.
    fn symbol_of(&self, t: &Tref) -> Symbol;

    /// Emit the declaration(s) for a struct / union / enum shape.
    fn emit_type(&self, shape: &Shape) -> Fragment;

    /// Emit an operation stub: a typed signature plus the opaque wire descriptor
    /// embedded as a blob and a `runtime.execute` call. The `descriptor` is
    /// never interpreted here.
    fn emit_op_stub(&self, op: &Shape, descriptor: &Value) -> Fragment;

    /// The `(protocol, language)` runtime package the generated SDK depends on,
    /// e.g. `"@sdk/http-runtime-ts"`.
    fn runtime_pkg(&self) -> &str;
}

/// How the shared component tree renders into a language's surface syntax. The
/// engine drives the pipeline (collecting imports, ordering, formatting); these
/// rules own only the language tokens, so the tree stays target-agnostic.
pub trait RenderRules {
    /// Render one collected import into an import statement.
    fn render_import(&self, import: &crate::codegen::symbol::Import) -> String;

    /// Render one declaration into rough but syntactically valid surface text.
    fn render_decl(&self, decl: &Decl) -> String;
}

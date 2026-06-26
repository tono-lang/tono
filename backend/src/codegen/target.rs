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
    /// Render one import statement for all names brought in from a single module.
    /// The names are deterministically ordered; a language that imports a whole
    /// package (Go) ignores them, while one with named imports (TypeScript, Rust)
    /// groups them into a single statement.
    fn render_import(&self, module: &str, names: &[&str]) -> String;

    /// Render one declaration into rough but syntactically valid surface text.
    fn render_decl(&self, decl: &Decl) -> String;
}

/// Declare a target's zero-sized struct and its [`Target`] impl from the
/// per-language pieces: the language name, its symbol table, its type emitter and
/// casing, and the runtime package. The `Target` surface is otherwise identical
/// across languages — `symbol_of`/`emit_type` delegate, operation stubs are
/// uniformly empty (owned by the runtime phase) — so this keeps that boilerplate
/// in one place, and adding a language is one invocation rather than a copied
/// impl.
#[macro_export]
macro_rules! declare_target {
    (
        $(#[$meta:meta])*
        $vis:vis struct $target:ident => {
            name: $name:expr,
            symbol_of: $symbol_of:path,
            emit_type: $emit_type:path,
            casing: $casing:path,
            runtime_pkg: $runtime:expr $(,)?
        }
    ) => {
        $(#[$meta])*
        $vis struct $target;

        impl $crate::codegen::target::Target for $target {
            fn name(&self) -> &str {
                $name
            }
            fn symbol_of(&self, t: &$crate::ir::Tref) -> $crate::codegen::symbol::Symbol {
                $symbol_of(t)
            }
            fn emit_type(&self, shape: &$crate::ir::Shape) -> $crate::codegen::target::Fragment {
                $emit_type(shape, &$casing())
            }
            // The opaque wire descriptor is never interpreted; stubs are owned by
            // the protocol/runtime work, so a target emits none here.
            fn emit_op_stub(
                &self,
                _op: &$crate::ir::Shape,
                _descriptor: &::serde_json::Value,
            ) -> $crate::codegen::target::Fragment {
                ::std::vec::Vec::new()
            }
            fn runtime_pkg(&self) -> &str {
                $runtime
            }
        }
    };
}

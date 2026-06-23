//! Per-language code generation targets. Each implements the engine's `Target`
//! and `RenderRules` traits with its own Symbol table and render rules; the
//! shared component tree, import collection, casing, and formatter come from the
//! engine.

pub mod typescript;

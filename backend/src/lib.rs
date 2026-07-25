//! Rust backend: typed codegen engine.
pub mod codegen;
pub mod compat;
pub mod config;
pub mod ir;

pub fn version() -> &'static str {
    "0.0.0"
}

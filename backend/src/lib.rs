//! Rust backend: typed codegen engine.
pub mod codegen;
pub mod compat;
mod compat_entry;
mod compat_shape;
pub mod config;
pub mod ir;

pub fn version() -> &'static str {
    "0.0.0"
}

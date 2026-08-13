//! Rust backend: typed codegen engine.
pub mod codegen;
pub mod compat;
mod compat_entry;
mod compat_shape;
pub mod config;
pub mod ir;
mod ir_extern_model;
mod ir_tests_model;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

//! The rendering harness `ext_call_tests.rs` and `ext_handle_call_tests.rs`
//! both need to turn a hand-built `Module` into generated TypeScript text,
//! re-exported once so neither sibling test module carries its own copy of
//! the same five imports.

pub(super) use super::emit;
pub(super) use crate::codegen::targets::typescript::types::ts_casing;
pub(super) use crate::codegen::targets::typescript::TsRules;
pub(super) use crate::codegen::test_support::{member, rendered, structure};

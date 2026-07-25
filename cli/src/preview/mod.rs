//! The `tono preview` command: a split-pane view of a `.tono` source and the SDK
//! it generates, with the generated code run through the target's native checker
//! so the pane also answers "does it compile?".
//!
//! [`pipeline`] is the pure chain (source to rendered-and-checked [`Snapshot`]);
//! [`tui`] is the terminal shell that drives it, watching the file and refreshing
//! on save; [`highlight`] colors the panes through each language's tree-sitter
//! grammar.

pub mod pipeline;

#[cfg(feature = "preview")]
pub mod highlight;
#[cfg(feature = "preview")]
pub mod tui;

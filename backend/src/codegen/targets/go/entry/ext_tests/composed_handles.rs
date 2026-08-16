//! The composed/injected-handle fixture ([`ext_fixtures::composed_handles_module`]),
//! checked through the full `emit` pipeline the same way `calls.rs` checks
//! the appendix fixture. The authoritative proof is `go_ext_roundtrip.rs`'s
//! `a_composed_and_an_injected_handle_both_build`, which actually `go
//! build`s the output; that test skips itself under coverage
//! instrumentation (no Go toolchain there), so this is what keeps the three
//! fixes covered either way.

use super::*;
use crate::codegen::targets::go::entry::ext_fixtures::composed_handles_model;

#[test]
fn the_composed_handles_model_pins_one_module_no_declared_tests() {
    let model = composed_handles_model();
    assert_eq!(model.tono_ir_version, crate::ir::TONO_IR_VERSION);
    assert_eq!(model.modules.len(), 1);
}

#[test]
fn a_composed_and_an_injected_handle_wire_all_three_fixes() {
    let module = crate::codegen::targets::go::entry::ext_fixtures::composed_handles_module();
    let text = entry_text(&module);

    // Bug 1: the adapter's own `real` field is the only storage position
    // that needs the foreign package's own import (a construction/storage
    // position spells tono's own generated interface instead, which is
    // local).
    assert!(text.contains("real *compose.Resource"), "{text}");

    // Bug 2: the constructor's `@arg` handle parameter and the Settings
    // field it feeds are spelled with the exact same (interface) type,
    // never the plain (undefined) package-local name.
    assert!(
        text.contains("injected composeResourceIface"),
        "constructor param must match the Settings field's storage type:\n{text}"
    );
    assert!(text.contains("\tinjected composeResourceIface\n"), "{text}");
    assert!(text.contains("\tprimary composeResourceIface\n"), "{text}");
    assert!(
        text.contains("\tsecondary composeResourceIface\n"),
        "{text}"
    );
    assert!(text.contains("\tcombined composeResourceIface\n"), "{text}");

    // Bug 3: primary/secondary are unwrapped back to the real library value
    // (never tono's own adapter) before being handed to NewCombined, the
    // library's own composition call.
    assert!(
        text.contains(
            "compose.NewCombined(s.B, s.primary.(*composeResourceIfaceAdapter).real, s.secondary.(*composeResourceIfaceAdapter).real)"
        ),
        "{text}"
    );

    // The two construction calls being composed still assign through the
    // adapter, same as every other handle-field construction.
    assert!(text.contains("s.primary = &composeResourceIfaceAdapter{real: primaryResult}"));
    assert!(text.contains("s.secondary = &composeResourceIfaceAdapter{real: secondaryResult}"));
    assert!(text.contains("s.combined = &composeResourceIfaceAdapter{real: combinedResult}"));

    // The op bodies read straight off the interface, injected or composed
    // alike: no foreign-specific handling left at the call site.
    assert!(text.contains("c.settings.combined.Get()"));
    assert!(text.contains("c.settings.injected.Get()"));
}

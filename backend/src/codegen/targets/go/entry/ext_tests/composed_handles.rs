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
    assert!(text.contains("\tcombined composeResourceIface\n"), "{text}");
    // A handle forwarded into another construction call is owned by that
    // call: the settings never hold it, so the ownership rule is visible in
    // the structure, not only in the validation.
    assert!(!text.contains("\tprimary composeResourceIface\n"), "{text}");
    assert!(
        !text.contains("\tsecondary composeResourceIface\n"),
        "{text}"
    );

    // Bug 3: primary/secondary reach NewCombined as the library's own values
    // (never tono's own adapter): each resolver returns the raw value, the
    // constructor binds it to a local and hands it to the consuming
    // resolver, whose parameters spell the library's type. No type
    // assertion anywhere.
    assert!(
        text.contains(
            "func resolvePrimary(a string, b string, c string, d string) (*compose.Resource, error) {"
        ),
        "{text}"
    );
    assert!(
        text.contains(
            "func resolveCombined(b string, primary *compose.Resource, secondary *compose.Resource) (*compose.Resource, error) {"
        ),
        "{text}"
    );
    assert!(
        text.contains("compose.NewCombined(b, primary, secondary)"),
        "{text}"
    );
    assert!(
        text.contains("primary, err := resolvePrimary(s.A, s.B, s.C, s.D)\n"),
        "{text}"
    );
    assert!(
        text.contains("secondary, err := resolveSecondary(s.B, s.C)\n"),
        "{text}"
    );
    assert!(
        text.contains("combined, err := resolveCombined(s.B, primary, secondary)\n"),
        "{text}"
    );
    assert!(!text.contains(").real"), "no unwrap anywhere: {text}");

    // Only the stored handle wraps into the adapter, at the site that stores it.
    assert!(text.contains("s.combined = &composeResourceIfaceAdapter{real: combined}"));
    assert!(!text.contains("s.primary ="), "{text}");
    assert!(!text.contains("s.secondary ="), "{text}");

    // The op bodies read straight off the interface, injected or composed
    // alike: no foreign-specific handling left at the call site.
    assert!(text.contains("c.settings.combined.Get()"));
    assert!(text.contains("c.settings.injected.Get()"));
}

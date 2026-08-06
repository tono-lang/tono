//! The boundary check helpers: construction-time `ConfigError`s, the
//! resolved-value casts the frozen `values` map needs, and the presence guard
//! around a frozen entry. The duration parser and env reader an entry's
//! resolution logic needs live in the SDK-root `duration`/`env` groups
//! ([`super::shared`]); a bytes-typed field reuses the `bytes` group's own
//! `base64_bytes::decode`, the same helper a wire struct field routes through
//! (see `HelperSet` in `rust/codecs.rs`) — an entry needs no copy of its own.

use super::*;

/// A construction-time failure returning the SDK's dedicated `ConfigError`
/// category as a full `return Err(..);` statement. `message_expr` is an
/// already-built Rust string expression (a `format!(..)` call or a literal).
/// Every bad env value, malformed blob, absent member, or unmatched select is
/// a config problem, discriminable via `TonoError::Config` from a transport,
/// validation, or declared error.
pub(super) fn config_error(message_expr: &str) -> String {
    format!("return Err(TonoError::Config(ConfigError {{ message: {message_expr} }}));")
}

/// The presence condition guarding a value entry, or `None` when the value is
/// always frozen (the decision lives in
/// [`crate::codegen::entries::needs_presence_guard`]; this only spells the
/// comparison). String-shaped targets compare against their empty/zero form;
/// `PartialEq` on the branded well-known types and the open enums makes this
/// comparison valid.
pub(super) fn presence_guard(
    entry: &EntryModel<'_>,
    vp: &crate::codegen::entries::ValuePath<'_>,
    expr: &str,
    module: &Module,
    config: &CasingConfig,
) -> Option<String> {
    crate::codegen::entries::needs_presence_guard(entry, vp)
        .then(|| format!("{expr} != {}", zero_value(vp.target, module, config)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_wraps_the_message_expression() {
        assert_eq!(
            config_error("\"bad\".to_string()"),
            "return Err(TonoError::Config(ConfigError { message: \"bad\".to_string() }));"
        );
    }
}

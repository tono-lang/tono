//! The consumed-chain "requires" leaf spellings for the Rust resolution
//! plan: a post-construction check that a consumed member/scalar actually
//! holds a value. Split out of `resolve.rs` to stay within the file-size
//! gate; self-contained (each takes only the pieces it needs, no shared
//! mutable state with the rest of the `Resolver`).

use super::resolve::Resolver;
use super::*;

pub(super) fn require_member(
    r: &Resolver<'_, '_>,
    head: &str,
    member: &str,
    leaf: &Tref,
    name: &str,
) -> String {
    let head_ident = field_snake_ren(
        head,
        r.entry.field_rename(head, LANG).as_deref(),
        r.config,
    );
    let member_ident = field_snake(member, r.config);
    let zero = zero_value(leaf, r.module, r.config);
    let msg = format!("{name}: no value");
    format!(
        "if s.{head_ident}.{member_ident} == {zero} {{\n    return Err(TonoError::Config(ConfigError {{ message: {msg:?}.to_string() }}));\n}}"
    )
}

pub(super) fn require_member_deferred(name: &str, err: &str) -> String {
    let e = err_var(err);
    format!(
        "if let Some({e}) = &{e} {{\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{name} <- {{}}\", {e}.message) }}));\n}}"
    )
}

pub(super) fn require_string(r: &Resolver<'_, '_>, head: &str, target: &Tref) -> String {
    let ident = field_snake_ren(
        head,
        r.entry.field_rename(head, LANG).as_deref(),
        r.config,
    );
    let zero = zero_value(target, r.module, r.config);
    let e = err_var(head);
    format!(
        "if s.{ident} == {zero} {{\n    let reason = {e}.as_ref().map(|err| err.message.clone()).unwrap_or_else(|| \"no value\".to_string());\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
    )
}

pub(super) fn require_bytes(r: &Resolver<'_, '_>, head: &str) -> String {
    let ident = field_snake_ren(
        head,
        r.entry.field_rename(head, LANG).as_deref(),
        r.config,
    );
    let e = err_var(head);
    format!(
        "if s.{ident}.is_empty() {{\n    let reason = {e}.as_ref().map(|err| err.message.clone()).unwrap_or_else(|| \"no value\".to_string());\n    return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", reason) }}));\n}}"
    )
}

pub(super) fn require_numeric(r: &Resolver<'_, '_>, head: &str, target: &Tref) -> String {
    let ident = field_snake_ren(
        head,
        r.entry.field_rename(head, LANG).as_deref(),
        r.config,
    );
    let e = err_var(head);
    let zero = numeric_zero(target);
    format!(
        "if let Some({e}) = &{e} {{\n    if s.{ident} == {zero} {{\n        return Err(TonoError::Config(ConfigError {{ message: format!(\"{head} <- {{}}\", {e}.message) }}));\n    }}\n}}"
    )
}

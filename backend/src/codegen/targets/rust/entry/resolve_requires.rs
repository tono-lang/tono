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

#[cfg(test)]
mod tests {
    use crate::codegen::targets::rust::rust_casing;
    use crate::codegen::test_support::bare_entry_field;
    use crate::ir::{EnvName, Shape, ShapeKind, Source, Trait};

    use super::*;

    fn module_of(shapes: Vec<Shape>) -> Module {
        Module {
            name: "m".into(),
            shapes,
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
            tests: vec![],
        }
    }

    fn client_shape(fields: Vec<EntryField>) -> Shape {
        Shape {
            id: "m#client".into(),
            kind: ShapeKind::Entry {
                fields,
                operations: vec![],
            },
            traits: vec![],
        }
    }

    /// A non-guaranteed top-level string/bytes/numeric field and a
    /// non-guaranteed config member of each kind, each consumed by an op's
    /// `@header` trait: this exercises every `require_*` leaf together
    /// (`require_string`, `require_bytes`, `require_numeric`,
    /// `require_member`, `require_member_deferred`) through the real
    /// resolution plan instead of calling the private leaf directly.
    #[test]
    fn every_require_leaf_emits_a_post_construction_check() {
        let phone = bare_entry_field(
            "phone",
            Tref::Prim(Prim::String),
            vec![Source::Env(EnvName::Name("PHONE".into()))],
        );
        let blob = bare_entry_field(
            "blob",
            Tref::Prim(Prim::Bytes),
            vec![Source::Env(EnvName::Name("BLOB".into()))],
        );
        let count = bare_entry_field(
            "count",
            Tref::Prim(Prim::I32),
            vec![Source::Env(EnvName::Name("COUNT".into()))],
        );
        let conf_shape = Shape {
            id: "m#conf".into(),
            kind: ShapeKind::Config {
                fields: vec![
                    bare_entry_field(
                        "email",
                        Tref::Prim(Prim::String),
                        vec![Source::Env(EnvName::Name("EMAIL".into()))],
                    ),
                    bare_entry_field(
                        "score",
                        Tref::Prim(Prim::I32),
                        vec![Source::Env(EnvName::Name("SCORE".into()))],
                    ),
                ],
            },
            traits: vec![],
        };
        let mut conf = bare_entry_field("conf", Tref::Prim(Prim::String), vec![]);
        conf.target = Tref::Ref {
            id: "m#conf".into(),
            args: vec![],
        };
        let op = Shape {
            id: "m#client.ping".into(),
            kind: ShapeKind::Operation {
                input_name: None,
                input: None,
                output: None,
                errors: vec![],
                wire: None,
                impl_call: None,
            },
            traits: vec![Trait {
                id: "header".into(),
                value: serde_json::json!([
                    {"field": ["phone"]},
                    {"field": ["blob"]},
                    {"field": ["count"]},
                    {"field": ["conf", "email"]},
                    {"field": ["conf", "score"]},
                ]),
            }],
        };
        let mut client = client_shape(vec![phone, blob, count, conf]);
        if let ShapeKind::Entry { operations, .. } = &mut client.kind {
            *operations = vec![op];
        }
        let module = module_of(vec![client, conf_shape]);
        let out = entry_text(&module, &rust_casing());
        assert!(
            out.contains("s.phone ==") || out.contains("phone <-"),
            "{out}"
        );
        assert!(out.contains("s.blob.is_empty()"), "{out}");
        assert!(out.contains("count_err"), "{out}");
        assert!(out.contains("s.conf.email =="), "{out}");
        assert!(out.contains("conf.score <-"), "{out}");
    }
}

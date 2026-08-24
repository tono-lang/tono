//! Entry and config diffing for the compatibility checker.
//!
//! Entries never cross the wire, so their categories are about the *generated
//! surface* and about *construction behavior*, not about decoding. Two rules
//! shape every classification here:
//!
//! - What a caller writes breaks when it disappears or changes arity: a removed
//!   `@arg`/`@with` field, a removed operation, a retyped field, a reordered
//!   argument list. Those are source breaks.
//! - Where a value comes from is behavior: an `@env` name, a `@default`, a
//!   `@format` template, a transform pipeline, a selection table, a binding.
//!   Changing one changes what the SDK resolves at construction without
//!   touching a single call site.

use std::collections::BTreeMap;

use crate::compat::{Category, Change};
use crate::compat_shape::render_tref;
use crate::ir::{EntryField, Shape, ShapeKind, Source, Trait};

/// Whether a field is part of the construction surface a caller writes: `@arg`
/// is positional, `@with` is an option. Everything else resolves on its own.
fn is_explicit(sources: &[Source]) -> bool {
    sources
        .iter()
        .any(|s| matches!(s, Source::Arg | Source::With))
}

fn has_arg(sources: &[Source]) -> bool {
    sources.iter().any(|s| matches!(s, Source::Arg))
}

/// The positional argument list, in declaration order: this *is* the generated
/// constructor signature.
fn arg_names(fields: &[EntryField]) -> Vec<&str> {
    fields
        .iter()
        .filter(|f| has_arg(&f.sources))
        .map(|f| f.name.as_str())
        .collect()
}

/// The surface a source chain exposes, ignoring where the fallbacks read from.
/// `@arg` and `@with` materialize as constructor parameters; the rest does not.
fn surface(sources: &[Source]) -> Vec<&'static str> {
    sources
        .iter()
        .filter_map(|s| match s {
            Source::Arg => Some("arg"),
            Source::With => Some("with"),
            Source::Env(_) | Source::Default(_) => None,
        })
        .collect()
}

fn is_deprecated(traits: &[Trait]) -> bool {
    traits
        .iter()
        .any(|t| t.id == "deprecated" || t.id == "core#deprecated")
}

/// An entry's fields and its operations.
pub(crate) fn diff_entry(
    id: &str,
    base_fields: &[EntryField],
    base_ops: &[Shape],
    curr_fields: &[EntryField],
    curr_ops: &[Shape],
    out: &mut Vec<Change>,
) {
    diff_fields(id, base_fields, curr_fields, out);
    diff_entry_ops(id, base_ops, curr_ops, out);
}

/// A config has fields but no operations; it is never constructed by the user,
/// so only its resolution behavior can change for them.
pub(crate) fn diff_config(
    id: &str,
    base_fields: &[EntryField],
    curr_fields: &[EntryField],
    out: &mut Vec<Change>,
) {
    diff_fields(id, base_fields, curr_fields, out);
}

fn diff_fields(id: &str, base: &[EntryField], curr: &[EntryField], out: &mut Vec<Change>) {
    let bi: BTreeMap<&str, &EntryField> = base.iter().map(|f| (f.name.as_str(), f)).collect();
    let ci: BTreeMap<&str, &EntryField> = curr.iter().map(|f| (f.name.as_str(), f)).collect();

    let mut names: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    names.sort_unstable();
    names.dedup();

    for name in names {
        let path = format!("{id}.{name}");
        match (bi.get(name), ci.get(name)) {
            (Some(b), None) => out.push(Change {
                key: format!("remove-field {path}"),
                category: if is_explicit(&b.sources) {
                    // The caller passed this one by hand; the call no longer compiles.
                    Category::SourceBreaking
                } else {
                    // An intermediate field: nothing named it from outside, but
                    // whatever consumed it now resolves from somewhere else.
                    Category::Behavioral
                },
                detail: "field removed".into(),
            }),
            (None, Some(c)) => out.push(Change {
                key: format!("add-field {path}"),
                category: if has_arg(&c.sources) {
                    // A new positional argument changes the constructor's arity.
                    Category::SourceBreaking
                } else {
                    Category::AdditiveSafe
                },
                detail: "field added".into(),
            }),
            (Some(b), Some(c)) => diff_field(&path, b, c, out),
            (None, None) => {}
        }
    }

    // Positional arguments are matched by position, so reordering them silently
    // rebinds every existing call.
    let before_args = arg_names(base);
    let after_args = arg_names(curr);
    if before_args != after_args
        && before_args.len() == after_args.len()
        && before_args.iter().all(|n| after_args.contains(n))
    {
        out.push(Change {
            key: format!("reorder-args {id}"),
            category: Category::SourceBreaking,
            detail: format!(
                "[{}] -> [{}]",
                before_args.join(", "),
                after_args.join(", ")
            ),
        });
    }
}

fn diff_field(path: &str, b: &EntryField, c: &EntryField, out: &mut Vec<Change>) {
    if b.target != c.target {
        out.push(Change {
            key: format!("retype-field {path}"),
            category: Category::SourceBreaking,
            detail: format!("{} -> {}", render_tref(&b.target), render_tref(&c.target)),
        });
    }

    if surface(&b.sources) != surface(&c.sources) {
        out.push(Change {
            key: format!("change-surface {path}"),
            category: Category::SourceBreaking,
            detail: format!(
                "[{}] -> [{}]",
                surface(&b.sources).join(", "),
                surface(&c.sources).join(", ")
            ),
        });
    } else if b.sources != c.sources {
        out.push(Change {
            key: format!("change-source {path}"),
            category: Category::Behavioral,
            detail: "value source chain changed".into(),
        });
    }

    if b.format != c.format {
        out.push(Change {
            key: format!("change-format {path}"),
            category: Category::Behavioral,
            detail: "format template changed".into(),
        });
    }

    if b.transforms != c.transforms {
        out.push(Change {
            key: format!("change-transforms {path}"),
            category: Category::Behavioral,
            detail: format!(
                "[{}] -> [{}]",
                b.transforms.join(", "),
                c.transforms.join(", ")
            ),
        });
    }

    if b.select != c.select {
        out.push(Change {
            key: format!("change-select {path}"),
            category: Category::Behavioral,
            detail: "selection table changed".into(),
        });
    }

    if b.binds != c.binds {
        out.push(Change {
            key: format!("change-bind {path}"),
            category: Category::Behavioral,
            detail: "composition binding changed".into(),
        });
    }

    crate::compat::diff_constraints(path, &b.constraints, &c.constraints, out);

    if !is_deprecated(&b.traits) && is_deprecated(&c.traits) {
        out.push(Change {
            key: format!("add-deprecated {path}"),
            category: Category::AdditiveSafe,
            detail: "marked deprecated".into(),
        });
    }
}

/// Operations declared in an entry body. They are keyed by their own shape id
/// (`module#entry.op`), never reach the top-level shape index, and so are
/// compared here.
fn diff_entry_ops(id: &str, base: &[Shape], curr: &[Shape], out: &mut Vec<Change>) {
    let bi: BTreeMap<&str, &Shape> = base.iter().map(|o| (o.id.as_str(), o)).collect();
    let ci: BTreeMap<&str, &Shape> = curr.iter().map(|o| (o.id.as_str(), o)).collect();

    let mut ids: Vec<&str> = bi.keys().chain(ci.keys()).copied().collect();
    ids.sort_unstable();
    ids.dedup();

    for op_id in ids {
        match (bi.get(op_id), ci.get(op_id)) {
            (Some(_), None) => out.push(Change {
                key: format!("remove-entry-op {op_id}"),
                category: Category::SourceBreaking,
                detail: format!("operation removed from {id}"),
            }),
            (None, Some(_)) => out.push(Change {
                key: format!("add-entry-op {op_id}"),
                category: Category::AdditiveSafe,
                detail: format!("operation added to {id}"),
            }),
            (Some(b), Some(c)) => {
                // wire and impl_call are deliberately excluded from this diff
                // for the same reason as compat::diff_shape_kind's Operation
                // arm: wire is derived from traits diff_descriptor below
                // already covers, and impl_call is bespoke implementation
                // detail, not part of the compared wire surface.
                if let (
                    ShapeKind::Operation {
                        input: bin,
                        input_name: _,
                        output: bout,
                        output_nullable: bnul,
                        errors: berr,
                        wire: _,
                        impl_call: _,
                    },
                    ShapeKind::Operation {
                        input: cin,
                        input_name: _,
                        output: cout,
                        output_nullable: cnul,
                        errors: cerr,
                        wire: _,
                        impl_call: _,
                    },
                ) = (&b.kind, &c.kind)
                {
                    crate::compat::diff_operation(
                        op_id, bin, bout, *bnul, berr, cin, cout, *cnul, cerr, out,
                    );
                }
                diff_descriptor(op_id, &b.traits, &c.traits, out);
            }
            (None, None) => {}
        }
    }
}

fn http_field<'a>(traits: &'a [Trait], key: &str) -> Option<&'a serde_json::Value> {
    traits
        .iter()
        .find(|t| t.id == "http" || t.id == "core#http")
        .and_then(|t| t.value.get(key))
}

fn has_http(traits: &[Trait]) -> bool {
    traits.iter().any(|t| t.id == "http" || t.id == "core#http")
}

fn protocol_trait<'a>(traits: &'a [Trait], name: &str) -> Option<&'a serde_json::Value> {
    traits
        .iter()
        .find(|t| t.id == name || t.id == format!("core#{name}"))
        .map(|t| &t.value)
}

/// The protocol vocabulary an operation carries. Method and path decide the
/// request that leaves the process, so changing one changes the wire. Endpoint,
/// headers, timeout and retry point at entry fields: they change what is
/// resolved, not the shape of the exchange.
///
/// An operation that gains or loses `@http` entirely is not compared: it moved
/// between a protocol and a bespoke `impl`, and an implemented operation is
/// judged by its contract, never by how it is implemented.
fn diff_descriptor(id: &str, base: &[Trait], curr: &[Trait], out: &mut Vec<Change>) {
    if !has_http(base) || !has_http(curr) {
        return;
    }

    for key in ["method", "path"] {
        let b = http_field(base, key);
        let c = http_field(curr, key);
        if b != c {
            out.push(Change {
                key: format!("change-http {id}${key}"),
                category: Category::WireBreaking,
                detail: format!(
                    "{key} {} -> {}",
                    render_json(b.unwrap_or(&serde_json::Value::Null)),
                    render_json(c.unwrap_or(&serde_json::Value::Null))
                ),
            });
        }
    }

    if http_field(base, "endpoint") != http_field(curr, "endpoint") {
        out.push(Change {
            key: format!("change-http {id}$endpoint"),
            category: Category::Behavioral,
            detail: "endpoint source changed".into(),
        });
    }

    for name in ["header", "timeout", "retry"] {
        if protocol_trait(base, name) != protocol_trait(curr, name) {
            out.push(Change {
                key: format!("change-descriptor {id}${name}"),
                category: Category::Behavioral,
                detail: format!("@{name} changed"),
            });
        }
    }
}

fn render_json(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => "none".into(),
        other => other.to_string(),
    }
}

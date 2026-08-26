//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! a resolver taking the handle through its generated interface (so the
//! same function runs over the real adapter or a test's fake), the `ctx`
//! slot filled at construction, the `@with` two-route shape around the
//! call, and the lookups a resolver is not built for. The end-to-end "does
//! the Go actually compile" proof is `go_ext_roundtrip::
//! a_field_sourced_from_a_handle_method_builds`; this module exercises the
//! emitter's own branches directly.

use super::*;
use crate::codegen::fixtures::handle_source::handle_source_module;
use crate::codegen::targets::go::entry::ext_resolver;

/// The resolver of the entry field named `field` of `module`, followed by
/// the constructor's call to it.
fn resolver_for(module: &Module, field_name: &str) -> String {
    let entries = module_entries(module);
    let entry = &entries[0];
    let field = entry
        .fields
        .iter()
        .copied()
        .find(|f| f.name == field_name)
        .expect("declared field");
    let config = go_casing();
    let decls: Vec<Decl> = ext_resolver::resolver_decls(entry, module, &config, false)
        .into_iter()
        .filter(|r| {
            r.decl.opaque_text().is_some_and(|t| {
                t.contains(&format!(
                    "func {}(",
                    ext_resolver::resolver_name(entry, field, false)
                ))
            })
        })
        .map(|r| r.decl)
        .collect();
    let mut text = rendered(&decls, &GoRules::default());
    text.push_str(&ext_resolver::call_site_from_settings(
        entry, module, &config, false, field,
    ));
    text
}

#[test]
fn a_ctx_marked_method_is_called_with_a_background_context_at_construction() {
    let module = handle_source_module("go");
    let text = resolver_for(&module, "config");
    assert!(
        text.contains(
            "func resolveConfig(provider envkitProviderIface) (Cfg, error) {\n\treturn provider.Get(context.Background())\n}"
        ),
        "{text}"
    );
    assert!(
        text.contains("config, err := resolveConfig(s.provider)\nif err != nil {\n\treturn nil, err\n}\ns.Config = config\n"),
        "{text}"
    );
}

#[test]
fn an_argument_reads_the_sibling_field_off_the_resolver_s_own_parameter() {
    let module = handle_source_module("go");
    let text = resolver_for(&module, "scoped");
    assert!(
        text.contains("func resolveScoped(provider envkitProviderIface, region string)"),
        "{text}"
    );
    assert!(text.contains("return provider.GetFor(region)"), "{text}");
    assert!(!text.contains("context.Background()"), "{text}");
    assert!(
        text.contains("scoped, err := resolveScoped(s.provider, s.Region)"),
        "{text}"
    );
}

#[test]
fn the_whole_entry_renders_the_field_source_and_the_op_reading_the_same_handle() {
    let module = handle_source_module("go");
    let text = entry_text(&module);
    assert!(text.contains("s.Config = config\n"), "{text}");
    assert!(
        text.contains("if w.scoped != nil {\n\t\ts.Scoped = *w.scoped\n\t} else {"),
        "{text}"
    );
    assert!(
        text.contains("c.settings.provider.GetFor(ctx, c.settings.Region)")
            || text.contains("c.settings.provider.GetFor(c.settings.Region)"),
        "{text}"
    );
}

#[test]
fn an_unresolved_handle_call_gets_no_resolver() {
    let mut module = handle_source_module("go");
    let set_call = |module: &mut Module, recv: &str, method: &str| {
        for shape in &mut module.shapes {
            if let ShapeKind::Entry { fields, .. } = &mut shape.kind {
                let f = fields.iter_mut().find(|f| f.name == "config").unwrap();
                f.handle_call = Some(OpImplCall {
                    recv: if recv.is_empty() {
                        vec![]
                    } else {
                        vec![recv.into()]
                    },
                    method: method.into(),
                    args: vec![],
                });
            }
        }
    };
    // No receiver, a receiver that is not a field, a receiver that is not a
    // handle, a method the handle does not declare, and a method with no
    // Go binding: the frontend refuses each before generation; the emitter
    // builds no resolver rather than trusting that, and the call site
    // still spells the call so a bypass fails `go build` loudly.
    set_call(&mut module, "", "get");
    assert!(!resolver_for(&module, "config").contains("func resolveConfig"));
    set_call(&mut module, "ghost", "get");
    assert!(!resolver_for(&module, "config").contains("func resolveConfig"));
    set_call(&mut module, "region", "get");
    assert!(!resolver_for(&module, "config").contains("func resolveConfig"));
    set_call(&mut module, "provider", "fetch");
    assert!(!resolver_for(&module, "config").contains("func resolveConfig"));

    // The method loses its Go binding.
    set_call(&mut module, "provider", "get");
    let mut no_go = module.clone();
    no_go.ext_libs[0].types[0].methods[0].langs.clear();
    let text = resolver_for(&no_go, "config");
    assert!(!text.contains("func resolveConfig"), "{text}");
    assert!(
        text.contains("config, err := resolveConfig(s.provider)"),
        "{text}"
    );
}

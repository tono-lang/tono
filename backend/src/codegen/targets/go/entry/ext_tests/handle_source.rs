//! A field sourced from a foreign handle's method (`= .field.method(args)`):
//! the interface call read off the resolved receiver, the `ctx` slot filled
//! at construction, the `@with` two-route shape around it, and the
//! defensive fallbacks each lookup degrades to. The end-to-end "does the
//! Go actually compile" proof is `go_ext_roundtrip::
//! a_field_sourced_from_a_handle_method_builds`; this module exercises the
//! emitter's own branches directly.

use super::*;
use crate::codegen::fixtures::handle_source::handle_source_module;
use crate::codegen::targets::go::entry::resolve::Resolver;
use crate::codegen::targets::go::entry::Helpers;

/// Run `handle_call_assign` for the entry field named `field` of `module`.
fn handle_call_assign_for(module: &Module, field_name: &str) -> String {
    let entries = module_entries(module);
    let entry = &entries[0];
    let field = entry
        .fields
        .iter()
        .copied()
        .find(|f| f.name == field_name)
        .expect("declared field");
    let config = go_casing();
    let overrides = std::collections::HashMap::new();
    let mut r = Resolver {
        entry,
        module,
        config: &config,
        helpers: &mut Helpers::default(),
        refs: &mut Vec::new(),
        body: &mut String::new(),
        resolve_fns: &mut Vec::new(),
        multi: false,
        overrides: &overrides,
    };
    let call = field.handle_call.clone().unwrap();
    ext::handle_call_assign(&mut r, field, &call, "s.Dest")
}

#[test]
fn a_ctx_marked_method_is_called_with_a_background_context_at_construction() {
    let module = handle_source_module("go");
    let text = handle_call_assign_for(&module, "config");
    assert!(
        text.contains("configOut, configErr := s.provider.Get(context.Background())"),
        "{text}"
    );
    assert!(
        text.contains("if configErr != nil {\n\treturn nil, configErr\n}"),
        "{text}"
    );
    assert!(text.contains("s.Dest = configOut"), "{text}");
}

#[test]
fn an_argument_reads_the_sibling_field_off_the_draft() {
    let module = handle_source_module("go");
    let text = handle_call_assign_for(&module, "scoped");
    assert!(
        text.contains("scopedOut, scopedErr := s.provider.GetFor(s.Region)"),
        "{text}"
    );
    assert!(!text.contains("context.Background()"), "{text}");
}

#[test]
fn the_whole_entry_renders_the_field_source_and_the_op_reading_the_same_handle() {
    let module = handle_source_module("go");
    let text = entry_text(&module);
    assert!(text.contains("s.Config = configOut"), "{text}");
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
fn handle_call_assign_degrades_on_every_unresolved_lookup() {
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
    set_call(&mut module, "", "get");
    assert!(handle_call_assign_for(&module, "config").contains("no receiver"));
    set_call(&mut module, "ghost", "get");
    assert!(handle_call_assign_for(&module, "config").contains("unresolved receiver field"));
    set_call(&mut module, "region", "get");
    assert!(handle_call_assign_for(&module, "config").contains("is not a foreign handle field"));
    set_call(&mut module, "provider", "fetch");
    assert!(handle_call_assign_for(&module, "config").contains("unresolved method"));

    // The method loses its Go binding.
    set_call(&mut module, "provider", "get");
    let mut no_go = module.clone();
    no_go.ext_libs[0].types[0].methods[0].langs.clear();
    assert!(handle_call_assign_for(&no_go, "config").contains("declares no Go binding"));
}

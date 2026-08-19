//! A generated TypeScript interface per opaque handle (`type X { extern
//! foo(..) }` inside an `ext` block), deriving each method's own signature
//! from exactly what the extern declaration itself says: a parameter's type
//! is an ordinary tono type (rendered the normal way), and a method's
//! return type is `Promise<unknown>` unless the method's own `ts` binding
//! declares a `yields` position naming a foreign struct, in which case that
//! struct's own verbatim shape becomes the return type. This is
//! deliberately not an adapter: nothing here projects a foreign value onto
//! a tono logical type (that stays Go's own design, where the interface
//! itself already speaks logical types); a handle interface speaks exactly
//! the foreign shape the extern declaration names, or admits it does not
//! know one.
//!
//! Emitted for every handle type across the module's own `ext` libs (not
//! only the ones a field or op body happens to reference): correct, unused
//! TypeScript costs nothing at generation time, and threading a
//! liveness set through every `field_ts_type`/`type_refs` call site this
//! module's naming feeds would cost real complexity for no observable
//! difference in the generated SDK.

use crate::codegen::tree::Decl;
use crate::ir::{ExtLib, ExternDecl, ForeignStruct, Module, OpaqueType};

use super::{camel, module_symbol, pascal, ts_type};

/// The handle type's own generated interface name: its local (unqualified)
/// name, cased and suffixed so it never collides with a same-named logical
/// shape the module also declares.
pub(super) fn handle_interface_name(id: &str) -> String {
    let local = id.rsplit('#').next().unwrap_or(id);
    format!("{}Handle", pascal(local))
}

/// A foreign struct's own generated companion type name.
fn foreign_struct_type_name(id: &str) -> String {
    let local = id.rsplit('#').next().unwrap_or(id);
    pascal(local)
}

/// Resolves a lib-qualified id (`"lib#name"`, the form an explicit
/// `lib.type` reference outside the `ext` block resolves to -- see
/// [`resolve_foreign_struct`] for the other scheme) to the `ExtLib` that
/// declares it and its handle type, or `None` if it names no handle
/// anywhere in the module (a broken invariant the frontend should already
/// have rejected).
pub(super) fn resolve_handle<'m>(
    id: &str,
    module: &'m Module,
) -> Option<(&'m ExtLib, &'m OpaqueType)> {
    module.ext_libs.iter().find_map(|lib| {
        lib.types
            .iter()
            .find(|t| format!("{}#{}", lib.name, t.name) == id)
            .map(|t| (lib, t))
    })
}

/// Resolves a module-qualified id to the `ForeignStruct` it names, or
/// `None` if it names no foreign struct anywhere in the module (a `yields`
/// position may instead name a foreign primitive or an opaque handle, which
/// this module has nothing to render a companion type for).
///
/// A bare reference written inside the `ext` block itself (`yields: (c:
/// kit_cfg)`) resolves through the frontend's own default (unqualified)
/// resolver, which namespaces by the *module*'s own name, not the `ext`
/// lib's: `"{module}#{name}"`, never `"{lib}#{name}"`. That differs from a
/// handle field's own declared type, written with an explicit qualifier
/// outside the block (`s: kit.src`), which the frontend resolves through the
/// *lib*-qualified path instead (`"{lib}#{name}"`, matching
/// `entries::is_foreign_ref` and this module's own [`resolve_handle`]).
/// Confirmed against the frontend's real emitted IR for both shapes, not
/// inferred from source alone.
pub(super) fn resolve_foreign_struct<'m>(
    id: &str,
    module: &'m Module,
) -> Option<&'m ForeignStruct> {
    let qualified = format!("{}#", module.name);
    let local = id.strip_prefix(&qualified)?;
    module
        .ext_libs
        .iter()
        .find_map(|lib| lib.structs.iter().find(|s| s.name == local))
}

/// The `ts`/`typescript` language block of a declared extern, if it has
/// one (a handle method with no TypeScript binding at all is simply absent
/// from the generated interface: nothing calls it from this target either).
pub(super) fn ts_lang(decl: &ExternDecl) -> Option<&crate::ir::ExternLang> {
    decl.langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
}

/// One handle method's own TS signature: `(params): Promise<Return>`. The
/// foreign struct id a `yields` position named, when there is one, is
/// reported back so the caller can queue its own companion type for
/// emission.
fn method_signature(
    decl: &ExternDecl,
    lang: &crate::ir::ExternLang,
    module: &Module,
) -> (String, Option<String>) {
    let params = decl
        .params
        .iter()
        .map(|p| format!("{}: {}", camel(&p.name), ts_type(&p.r#type)))
        .collect::<Vec<_>>()
        .join(", ");
    let yields_type = lang
        .yields
        .iter()
        .find(|y| !y.is_error)
        .and_then(|y| y.r#type.as_ref());
    let (ret, struct_id) = match yields_type {
        Some(crate::ir::Tref::Ref { id, .. }) if resolve_foreign_struct(id, module).is_some() => {
            (foreign_struct_type_name(id), Some(id.clone()))
        }
        // A yields position naming anything else (a foreign primitive, an
        // opaque handle) or no yields position at all: the raw call result
        // has no struct shape this module can name a companion type for,
        // so `unknown` is the honest answer.
        _ => ("unknown".to_string(), None),
    };
    // `lang.symbol` is the real method name on the foreign/handle object
    // (whatever the actual runtime call site invokes), never cased: an
    // interface member key must match it verbatim, or the generated call
    // site (`recv.{symbol}(..)`) would no longer type-check against this
    // same interface.
    (
        format!("{}({params}): Promise<{ret}>;", lang.symbol),
        struct_id,
    )
}

/// A handle's own generated interface: one method per declared method
/// carrying a `ts` binding. `foreign_struct_ids` collects every struct id a
/// method's own return type named, so the caller emits exactly the
/// companion types this interface (and no other) actually references.
pub(super) fn handle_interface_decl(
    lib: &ExtLib,
    handle: &OpaqueType,
    module: &Module,
    foreign_struct_ids: &mut std::collections::BTreeSet<String>,
) -> Decl {
    let name = handle_interface_name(&format!("{}#{}", lib.name, handle.name));
    let mut methods = String::new();
    let mut refs = Vec::new();
    for decl in &handle.methods {
        let Some(lang) = ts_lang(decl) else { continue };
        let (sig, struct_id) = method_signature(decl, lang, module);
        methods.push_str(&format!("  {sig}\n"));
        if let Some(id) = struct_id {
            refs.push(module_symbol(&foreign_struct_type_name(&id), module));
            foreign_struct_ids.insert(id);
        }
    }
    Decl::raw_with(
        format!(
            "// {name} is the generated shape of the `{handle_name}` opaque handle's\n\
             // own declared methods: a parameter is an ordinary tono type, a method's\n\
             // return type is `unknown` unless its own `ts` binding declares a\n\
             // `yields` position naming a foreign struct, whose verbatim shape is\n\
             // used instead. Never the logical projection a `returns:` clause would\n\
             // compute -- this interface speaks exactly what the foreign call itself\n\
             // returns, nothing more.\n\
             export interface {name} {{\n{methods}}}",
            handle_name = handle.name,
        ),
        refs,
    )
}

/// A foreign struct's own generated companion type: its declared fields,
/// verbatim (never cased -- these are the real third-party field names, the
/// same convention `raw.<field>` projection already reads them with).
pub(super) fn foreign_struct_decl(strct: &ForeignStruct, module: &Module, id: &str) -> Decl {
    let name = foreign_struct_type_name(id);
    let mut refs = Vec::new();
    let mut fields = String::new();
    for f in &strct.fields {
        refs.extend(super::type_refs(&f.r#type, module));
        fields.push_str(&format!("  {}: {};\n", f.name, ts_type(&f.r#type)));
    }
    Decl::raw_with(
        format!(
            "// {name} is the verbatim shape of the foreign `{struct_name}` struct a\n\
             // handle method's own `yields` position names: field names ride exactly\n\
             // as the third-party library declares them, uncased.\n\
             export interface {name} {{\n{fields}}}",
            struct_name = strct.name,
        ),
        refs,
    )
}

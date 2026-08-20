//! Compatibility diffing for `ext`/`extern` FFI library blocks.
//!
//! Two things escape the ordinary shape/entry diff entirely, because neither
//! shows up in the tono-facing signature the rest of the checker compares:
//!
//! - The FFI call shape itself (receiver type, foreign symbol, argument order, `yields`,
//!   `returns` projection, sentinel-to-error mapping). Swapping `"Load"` for
//!   `"LoadV2"` with the arguments reordered does not touch the `extern`'s
//!   declared params/return, so nothing else would ever notice.
//! - The pinned third-party version in the manifest, which is not part of the
//!   IR (a `Model`) at all.

use std::collections::BTreeMap;

use crate::compat::{Category, Change};
use crate::ir::{ExtLib, ExternDecl, ExternLang, Model, Module};

/// Compare `baseline` against `current`'s `ext` blocks, appending a [`Change`]
/// for every FFI binding whose call shape changed or disappeared for a
/// language it used to support. Additions (a new `ext`, a new `extern`, a new
/// language binding) are pure growth and are not reported, mirroring how
/// [`crate::compat::diff`]'s taxonomy pass only tracks prunes.
pub fn diff_ext_libs(baseline: &Model, current: &Model, out: &mut Vec<Change>) {
    let curr_modules: BTreeMap<&str, &Module> = current
        .modules
        .iter()
        .map(|m| (m.name.as_str(), m))
        .collect();
    for module in &baseline.modules {
        let Some(curr_module) = curr_modules.get(module.name.as_str()) else {
            continue;
        };
        let curr_libs: BTreeMap<&str, &ExtLib> = curr_module
            .ext_libs
            .iter()
            .map(|l| (l.name.as_str(), l))
            .collect();
        for lib in &module.ext_libs {
            let Some(curr_lib) = curr_libs.get(lib.name.as_str()) else {
                continue;
            };
            diff_externs(
                &module.name,
                lib,
                curr_lib,
                &named_externs(lib),
                &named_externs(curr_lib),
                out,
            );
            diff_instances(&module.name, lib, curr_lib, out);
        }
    }
}

/// An opaque type's instantiation (the foreign name and argument it
/// monomorphizes) changing, appearing, or disappearing between two versions
/// of the same handle: the emitted Go type spelling
/// (`*pkg.Source[Settings]`) changes with it, so generated consumer code
/// that names the concrete type no longer compiles.
fn diff_instances(module: &str, lib: &ExtLib, curr_lib: &ExtLib, out: &mut Vec<Change>) {
    let curr_types: BTreeMap<&str, &crate::ir_extern_model::OpaqueType> = curr_lib
        .types
        .iter()
        .map(|t| (t.name.as_str(), t))
        .collect();
    for ty in &lib.types {
        let Some(curr_ty) = curr_types.get(ty.name.as_str()) else {
            continue;
        };
        if ty.instance != curr_ty.instance {
            out.push(Change {
                key: format!("ext-instance {module}::{}.{}", lib.name, ty.name),
                category: Category::SourceBreaking,
                detail: format!(
                    "{}.{}: the foreign instantiation changed; the emitted concrete \
                     type no longer matches generated consumer code that names it",
                    lib.name, ty.name
                ),
            });
        }
        // The interface marker flips the emitted Go spelling between a
        // pointer and an interface value (`*pkg.T` vs `pkg.T`), the same
        // consumer-facing surface the instantiation rule above guards.
        if ty.interface != curr_ty.interface {
            out.push(Change {
                key: format!("ext-shape {module}::{}.{}", lib.name, ty.name),
                category: Category::SourceBreaking,
                detail: format!(
                    "{}.{}: the handle's foreign shape changed between concrete \
                     and interface; the emitted type spelling (pointer vs value) \
                     no longer matches generated consumer code that names it",
                    lib.name, ty.name
                ),
            });
        }
    }
}

/// Every `extern` an `ext` block declares, free functions and opaque-handle
/// methods alike, keyed the same way the declared-test stub grammar already
/// qualifies them (`stub <ext>.<name>` / `stub <ext>.<type>.<method>`), so a
/// method and a free function of the same name never collide.
fn named_externs(lib: &ExtLib) -> BTreeMap<String, &ExternDecl> {
    let mut named = BTreeMap::new();
    for decl in &lib.externs {
        named.insert(decl.name.clone(), decl);
    }
    for ty in &lib.types {
        for decl in &ty.methods {
            named.insert(format!("{}.{}", ty.name, decl.name), decl);
        }
    }
    named
}

fn diff_externs(
    module: &str,
    lib: &ExtLib,
    curr_lib: &ExtLib,
    base: &BTreeMap<String, &ExternDecl>,
    curr: &BTreeMap<String, &ExternDecl>,
    out: &mut Vec<Change>,
) {
    for (name, decl) in base {
        let Some(curr_decl) = curr.get(name) else {
            continue;
        };
        let curr_langs: BTreeMap<&str, &ExternLang> = curr_decl
            .langs
            .iter()
            .map(|l| (l.lang.as_str(), l))
            .collect();
        for lang in &decl.langs {
            let key_base = format!("ext-call {module}::{}.{name}@{}", lib.name, lang.lang);
            match curr_langs.get(lang.lang.as_str()) {
                None => out.push(Change {
                    key: key_base,
                    category: Category::SourceBreaking,
                    detail: format!(
                        "{}.{name}@{}: the {} binding was removed; \
                         source generated against it no longer compiles",
                        lib.name, lang.lang, lang.lang
                    ),
                }),
                Some(curr_lang) if *curr_lang != lang => out.push(Change {
                    key: key_base,
                    category: Category::Behavioral,
                    detail: format!(
                        "{}.{name}@{}: the foreign call binding changed (receiver type, symbol, \
                         argument order, yields, returns, or error mapping); the tono-facing \
                         signature and the wire are unaffected, but runtime behavior is not",
                        curr_lib.name, lang.lang
                    ),
                }),
                Some(_) => {}
            }
        }
    }
}

/// Compare pinned `[ext.<name>]` versions between two manifests. Only a
/// changed pin is reported (not an added or removed one): a fresh pin has
/// nothing on the other side to have broken, and the case this guards
/// against is a pinned third-party library bumping version underneath the
/// generated SDK.
pub fn diff_ext_versions(
    baseline: &BTreeMap<String, BTreeMap<String, String>>,
    current: &BTreeMap<String, BTreeMap<String, String>>,
) -> Vec<Change> {
    let mut out = Vec::new();
    for (name, langs) in baseline {
        let Some(curr_langs) = current.get(name) else {
            continue;
        };
        for (lang, version) in langs {
            let Some(curr_version) = curr_langs.get(lang) else {
                continue;
            };
            if curr_version != version {
                out.push(Change {
                    key: format!("ext-version-bumped {name}@{lang}"),
                    category: Category::Behavioral,
                    detail: format!(
                        "{name}@{lang}: pinned version changed from {version} to {curr_version}; \
                         the .tono source did not change, but what the third-party library does might"
                    ),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{CallArg, ExternParam, LangPath, Model, Module, OpaqueType, Tref};

    fn tref(name: &str) -> Tref {
        Tref::Ref {
            id: name.to_string(),
            args: vec![],
        }
    }

    fn model(module: Module) -> Model {
        Model {
            tono_ir_version: 1,
            modules: vec![module],
        }
    }

    fn base_module() -> Module {
        Module {
            name: "example".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![],
            tests: vec![],
            ext_libs: vec![ExtLib {
                name: "companyconfig".into(),
                langs: vec![LangPath {
                    lang: "go".into(),
                    path: "github.com/company/config".into(),
                }],
                structs: vec![],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "load".into(),
                    params: vec![ExternParam {
                        variadic: false,
                        name: "service".into(),
                        r#type: tref("string"),
                    }],
                    r#return: tref("app_config"),
                    langs: vec![ExternLang {
                        lang: "go".into(),
                        symbol: "Load".into(),
                        call_args: vec![CallArg::Param("service".into())],
                        yields: vec![],
                        returns: None,
                        errors: vec![],
                        sync: false,
                        infallible: false,
                        ctx: false,
                        receiver: None,
                        is_new: false,
                    }],
                }],
            }],
        }
    }

    #[test]
    fn unrelated_models_report_nothing() {
        let m = base_module();
        let base = model(m.clone());
        let curr = model(m);
        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_swapped_symbol_and_argument_order_is_behavioral() {
        let base = model(base_module());
        let mut changed = base_module();
        let lang = &mut changed.ext_libs[0].externs[0].langs[0];
        lang.symbol = "LoadV2".into();
        lang.call_args = vec![
            CallArg::Param("region".into()),
            CallArg::Param("service".into()),
        ];
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::Behavioral);
        assert!(
            out[0].key.contains("companyconfig.load@go"),
            "{}",
            out[0].key
        );
    }

    #[test]
    fn a_removed_lang_binding_is_source_breaking() {
        let base = model(base_module());
        let mut changed = base_module();
        changed.ext_libs[0].externs[0].langs.clear();
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::SourceBreaking);
    }

    #[test]
    fn a_method_extern_on_an_opaque_type_is_compared_too() {
        let mut m = base_module();
        m.ext_libs[0].externs.clear();
        m.ext_libs[0].types = vec![OpaqueType {
            name: "publisher".into(),
            interface: false,
            instance: None,
            methods: vec![ExternDecl {
                name: "send".into(),
                params: vec![],
                r#return: tref("ack"),
                langs: vec![ExternLang {
                    lang: "go".into(),
                    symbol: "Send".into(),
                    call_args: vec![],
                    yields: vec![],
                    returns: None,
                    errors: vec![],
                    sync: false,
                    infallible: false,
                    ctx: false,
                    receiver: None,
                    is_new: false,
                }],
            }],
        }];
        let base = model(m.clone());
        let mut changed = m;
        changed.ext_libs[0].types[0].methods[0].langs[0].symbol = "SendV2".into();
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert_eq!(out.len(), 1);
        assert!(out[0].key.contains("publisher.send@go"), "{}", out[0].key);
    }

    #[test]
    fn a_changed_instantiation_is_source_breaking() {
        let mut m = base_module();
        m.ext_libs[0].externs.clear();
        m.ext_libs[0].types = vec![OpaqueType {
            name: "env_source".into(),
            interface: false,
            instance: Some(crate::ir_extern_model::Instance {
                names: vec![crate::ir_extern_model::InstanceName {
                    lang: "go".into(),
                    name: "Source".into(),
                }],
                arg: tref("settings"),
            }),
            methods: vec![],
        }];
        let base = model(m.clone());
        let mut changed = m;
        changed.ext_libs[0].types[0].instance.as_mut().unwrap().arg = tref("other_settings");
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::SourceBreaking);
        assert!(out[0].key.contains("env_source"), "{}", out[0].key);
    }

    #[test]
    fn a_flipped_interface_marker_is_source_breaking() {
        let mut m = base_module();
        m.ext_libs[0].externs.clear();
        m.ext_libs[0].types = vec![OpaqueType {
            name: "meter".into(),
            interface: false,
            instance: None,
            methods: vec![],
        }];
        let base = model(m.clone());
        let mut changed = m;
        changed.ext_libs[0].types[0].interface = true;
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::SourceBreaking);
        assert!(out[0].key.contains("ext-shape"), "{}", out[0].key);
        assert!(out[0].key.contains("meter"), "{}", out[0].key);
    }

    #[test]
    fn an_unchanged_instantiation_draws_no_diagnostic() {
        let mut m = base_module();
        m.ext_libs[0].externs.clear();
        m.ext_libs[0].types = vec![OpaqueType {
            name: "env_source".into(),
            interface: false,
            instance: Some(crate::ir_extern_model::Instance {
                names: vec![crate::ir_extern_model::InstanceName {
                    lang: "go".into(),
                    name: "Source".into(),
                }],
                arg: tref("settings"),
            }),
            methods: vec![],
        }];
        let base = model(m.clone());
        let curr = model(m);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn version_bump_is_reported_once_as_behavioral() {
        let mut baseline = BTreeMap::new();
        baseline.insert(
            "companyconfig".to_string(),
            BTreeMap::from([("go".to_string(), "v1.4.2".to_string())]),
        );
        let mut current = baseline.clone();
        current
            .get_mut("companyconfig")
            .unwrap()
            .insert("go".to_string(), "v1.5.0".to_string());

        let out = diff_ext_versions(&baseline, &current);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].category, Category::Behavioral);
        assert!(out[0].detail.contains("v1.4.2"));
        assert!(out[0].detail.contains("v1.5.0"));
    }

    #[test]
    fn an_added_or_removed_pin_is_not_reported() {
        let mut baseline = BTreeMap::new();
        baseline.insert(
            "companyconfig".to_string(),
            BTreeMap::from([("go".to_string(), "v1.4.2".to_string())]),
        );
        let mut current = baseline.clone();
        current
            .get_mut("companyconfig")
            .unwrap()
            .insert("ts".to_string(), "^3.1.0".to_string());
        current.insert(
            "companybus".to_string(),
            BTreeMap::from([("go".to_string(), "v0.1.0".to_string())]),
        );

        let out = diff_ext_versions(&baseline, &current);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_removed_pin_from_the_baseline_side_is_not_reported() {
        let mut baseline = BTreeMap::new();
        baseline.insert(
            "companyconfig".to_string(),
            BTreeMap::from([
                ("go".to_string(), "v1.4.2".to_string()),
                ("ts".to_string(), "^3.1.0".to_string()),
            ]),
        );
        baseline.insert(
            "companybus".to_string(),
            BTreeMap::from([("go".to_string(), "v0.1.0".to_string())]),
        );
        // Current drops the "ts" pin and the whole "companybus" ext entirely.
        let current = BTreeMap::from([(
            "companyconfig".to_string(),
            BTreeMap::from([("go".to_string(), "v1.4.2".to_string())]),
        )]);

        let out = diff_ext_versions(&baseline, &current);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_removed_extern_is_not_reported_by_the_call_shape_diff() {
        // Removing the whole `extern` (not just a lang binding) is not this
        // pass's concern: the extern's own params/return leaving the surface
        // is covered by the ordinary shape diff elsewhere, not here.
        let base = model(base_module());
        let mut changed = base_module();
        changed.ext_libs[0].externs.clear();
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[test]
    fn a_removed_ext_lib_is_not_reported_by_the_call_shape_diff() {
        let base = model(base_module());
        let mut changed = base_module();
        changed.ext_libs.clear();
        let curr = model(changed);

        let mut out = Vec::new();
        diff_ext_libs(&base, &curr, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}

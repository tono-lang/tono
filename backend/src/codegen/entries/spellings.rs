//! Every foreign spelling an `ext` block carries, by the site that declares
//! it: what a rule over spellings (a reference that must resolve, a probe
//! that cannot express a generated type) walks instead of each knowing the
//! IR's shape of a binding.

use crate::ir::{CallArg, ExtLib, ExternLang, ForeignLang, Module};

/// The spellings inside a call's argument tree: a parameter's own spelling,
/// a declared position, a nested symbol call's callee (and its own
/// arguments), a struct literal's own spelling and its arguments, a list's
/// items.
pub(crate) fn of_args<'a>(args: &'a [CallArg], out: &mut Vec<&'a str>) {
    for arg in args {
        match arg {
            CallArg::ParamAs { spelling, .. } | CallArg::Foreign(spelling) => out.push(spelling),
            CallArg::SymbolCall(sc) => {
                out.push(&sc.symbol);
                of_args(&sc.args, out);
            }
            CallArg::Ctor(c) => {
                out.extend(c.spelling.as_deref());
                for v in c.fields.values() {
                    of_args(std::slice::from_ref(v), out);
                }
            }
            CallArg::List(items) => of_args(items, out),
            CallArg::Param(_) | CallArg::Ref(_) | CallArg::Lit(_) | CallArg::TypeRef(_) => {}
            CallArg::Call(call) => of_args(&call.args, out),
        }
    }
}

/// The spellings of one language block of an op: the callee, every
/// spelling in its arguments, every `yields` position spelled foreign.
pub(crate) fn of_lang(lang: &ExternLang) -> Vec<&str> {
    let mut out = vec![lang.symbol.as_str()];
    of_args(&lang.call_args, &mut out);
    out.extend(lang.yields.iter().filter_map(|y| y.foreign.as_deref()));
    out
}

/// The spellings of a struct's language block: the foreign name and every
/// spelled field.
pub(crate) fn of_block(block: &ForeignLang) -> Vec<&str> {
    std::iter::once(block.name.as_str())
        .chain(block.fields.values().map(String::as_str))
        .collect()
}

/// Every spelling of `lib`, each with the site it belongs to (the way a
/// diagnostic names it) and the language block it sits in; then the
/// spellings of the module's error structs, which recognize the library's
/// failures through their own `foreign` blocks.
pub(crate) fn of_lib(lib: &ExtLib) -> Vec<(String, &str, &str)> {
    let mut out = Vec::new();
    for handle in &lib.types {
        for block in &handle.langs {
            let site = format!("handle {}.{}", lib.name, handle.name);
            out.extend(
                of_block(block)
                    .into_iter()
                    .map(|sp| (site.clone(), block.lang.as_str(), sp)),
            );
        }
        for method in &handle.methods {
            for lang in &method.langs {
                let site = format!("{}.{}.{}", lib.name, handle.name, method.name);
                out.extend(
                    of_lang(lang)
                        .into_iter()
                        .map(|sp| (site.clone(), lang.lang.as_str(), sp)),
                );
            }
        }
    }
    for form in &lib.structs {
        for block in &form.langs {
            let site = format!("struct {}.{}", lib.name, form.name);
            out.extend(
                of_block(block)
                    .into_iter()
                    .map(|sp| (site.clone(), block.lang.as_str(), sp)),
            );
        }
    }
    for decl in &lib.externs {
        for lang in &decl.langs {
            let site = format!("{}.{}", lib.name, decl.name);
            out.extend(
                of_lang(lang)
                    .into_iter()
                    .map(|sp| (site.clone(), lang.lang.as_str(), sp)),
            );
        }
    }
    out
}

/// The spellings of the module's error structs' `foreign` blocks.
pub(crate) fn of_errors(module: &Module) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for shape in &module.shapes {
        for block in ForeignLang::of_shape(shape) {
            let site = format!("error {}", super::local_name(&shape.id));
            for sp in of_block(&block) {
                out.push((site.clone(), block.lang.clone(), sp.to_string()));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        CallCtor, EntryCall, ExternDecl, ExternParam, ForeignStruct, OpaqueType, Prim, SymbolCall,
        Tref, YieldsPos,
    };
    use std::collections::BTreeMap;

    fn lang(symbol: &str, args: Vec<CallArg>) -> ExternLang {
        ExternLang {
            lang: "go".into(),
            symbol: symbol.into(),
            call_args: args,
            yields: vec![YieldsPos {
                name: "raw".into(),
                r#type: None,
                is_error: false,
                foreign: Some("[]byte".into()),
            }],
            returns: None,
        }
    }

    #[test]
    fn every_spelling_position_of_a_call_is_walked() {
        let mut ctor_fields = BTreeMap::new();
        ctor_fields.insert(
            "n".to_string(),
            CallArg::ParamAs {
                name: "n".into(),
                spelling: "int".into(),
            },
        );
        let l = lang(
            "Open[.reading]",
            vec![
                CallArg::Param("a".into()),
                CallArg::ParamAs {
                    name: "b".into(),
                    spelling: "[]float64".into(),
                },
                CallArg::Foreign("ctx context.Context".into()),
                CallArg::SymbolCall(SymbolCall {
                    symbol: "WithPrecision".into(),
                    args: vec![CallArg::Lit(serde_json::json!(4))],
                }),
                CallArg::Ctor(CallCtor {
                    name: "opts".into(),
                    fields: ctor_fields,
                    spelling: Some("&Options".into()),
                }),
                CallArg::List(vec![
                    CallArg::Ref(vec!["x".into()]),
                    CallArg::TypeRef("h".into()),
                ]),
                CallArg::Call(Box::new(EntryCall {
                    ns: "ns".into(),
                    func: "f".into(),
                    args: vec![CallArg::ParamAs {
                        name: "c".into(),
                        spelling: "uint8".into(),
                    }],
                })),
            ],
        );
        assert_eq!(
            of_lang(&l),
            [
                "Open[.reading]",
                "[]float64",
                "ctx context.Context",
                "WithPrecision",
                "&Options",
                "int",
                "uint8",
                "[]byte"
            ]
        );
    }

    #[test]
    fn every_site_of_a_lib_is_walked_with_its_name() {
        let mut fields = BTreeMap::new();
        fields.insert("n".to_string(), "Option<u8>".to_string());
        let lib = ExtLib {
            name: "kit".into(),
            langs: vec![],
            structs: vec![ForeignStruct {
                name: "opts".into(),
                fields: vec![],
                langs: vec![ForeignLang {
                    lang: "rust".into(),
                    name: "Opts".into(),
                    fields,
                }],
            }],
            types: vec![OpaqueType {
                name: "memo".into(),
                langs: vec![ForeignLang {
                    lang: "go".into(),
                    name: "*Memo[.reading]".into(),
                    fields: BTreeMap::new(),
                }],
                methods: vec![ExternDecl {
                    name: "recall".into(),
                    params: vec![],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![lang("Recall", vec![])],
                    r#async: vec![],
                    errors: vec![],
                }],
            }],
            externs: vec![ExternDecl {
                name: "remember".into(),
                params: vec![ExternParam {
                    name: "seed".into(),
                    r#type: Tref::Prim(Prim::String),
                }],
                r#return: Tref::Prim(Prim::String),
                langs: vec![lang(
                    "Remember[.reading]",
                    vec![CallArg::Param("seed".into())],
                )],
                r#async: vec![],
                errors: vec![],
            }],
        };
        let sites: Vec<(String, &str, &str)> = of_lib(&lib);
        assert_eq!(
            sites,
            [
                ("handle kit.memo".to_string(), "go", "*Memo[.reading]"),
                ("kit.memo.recall".to_string(), "go", "Recall"),
                ("kit.memo.recall".to_string(), "go", "[]byte"),
                ("struct kit.opts".to_string(), "rust", "Opts"),
                ("struct kit.opts".to_string(), "rust", "Option<u8>"),
                ("kit.remember".to_string(), "go", "Remember[.reading]"),
                ("kit.remember".to_string(), "go", "[]byte"),
            ]
        );
    }

    #[test]
    fn error_structs_contribute_their_foreign_blocks() {
        let mut module = Module {
            name: "m".into(),
            shapes: vec![],
            operations: vec![],
            extensions: vec![],
            ext_libs: vec![],
            tests: vec![],
        };
        let mut fields = BTreeMap::new();
        fields.insert("message".to_string(), "Error()".to_string());
        module.shapes.push(crate::ir::Shape {
            id: "m#timed_out".into(),
            kind: crate::ir::ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits: vec![crate::ir::Trait {
                id: "foreign".into(),
                value: serde_json::to_value(vec![ForeignLang {
                    lang: "go".into(),
                    name: "*TimeoutError".into(),
                    fields,
                }])
                .unwrap(),
            }],
        });
        assert_eq!(
            of_errors(&module),
            [
                (
                    "error timed_out".to_string(),
                    "go".to_string(),
                    "*TimeoutError".to_string()
                ),
                (
                    "error timed_out".to_string(),
                    "go".to_string(),
                    "Error()".to_string()
                ),
            ]
        );
        assert!(of_errors(&Module {
            shapes: vec![],
            ..module
        })
        .is_empty());
    }
}

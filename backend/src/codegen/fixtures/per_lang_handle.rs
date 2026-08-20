//! A worked example of one logical handle naming a *different* foreign type
//! per language: the `keepkit` stand-in exports the same generic contract as
//! `Store[T]` in Go and `Vault<T>` in Rust, both concrete types with
//! inherent methods, and the handle's instantiation carries one name per
//! language. The compiled-vector checks (`go_ext_roundtrip.rs`,
//! `rust_ext_roundtrip.rs`) build the generated SDK against each stand-in,
//! which is what proves both spellings resolve, beyond rendered text.
//!
//! Not `cfg(test)`: the integration tests link it from outside the crate.

use crate::ir::{
    CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam, Instance,
    InstanceName, LangPath, Model, Module, OpImplCall, OpaqueType, Prim, Shape, ShapeKind, Source,
    Trait, Tref, TONO_IR_VERSION,
};

fn string_t() -> Tref {
    Tref::Prim(Prim::String)
}

fn reference(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
}

fn plain_lang(lang: &str, symbol: &str, call_args: Vec<CallArg>, ctx: bool) -> ExternLang {
    ExternLang {
        lang: lang.into(),
        symbol: symbol.into(),
        call_args,
        yields: vec![],
        returns: None,
        errors: vec![],
        sync: false,
        infallible: false,
        ctx,
        is_new: false,
    }
}

/// The `keepkit` library: a `locker` handle instantiating a foreign generic
/// whose exported identifier differs per language (`Store` in Go, `Vault`
/// in Rust), with one inherent method and a free constructor.
fn keepkit() -> ExtLib {
    ExtLib {
        name: "keepkit".into(),
        langs: vec![
            LangPath {
                lang: "go".into(),
                path: "tono-ext-fixture/keepkit".into(),
            },
            LangPath {
                lang: "rust".into(),
                path: "keepkit".into(),
            },
        ],
        structs: vec![],
        types: vec![OpaqueType {
            name: "locker".into(),
            interface: false,
            instance: Some(Instance {
                names: vec![
                    InstanceName {
                        lang: "go".into(),
                        name: "Store".into(),
                    },
                    InstanceName {
                        lang: "rust".into(),
                        name: "Vault".into(),
                    },
                ],
                arg: string_t(),
            }),
            methods: vec![ExternDecl {
                name: "get".into(),
                params: vec![],
                r#return: string_t(),
                langs: vec![
                    plain_lang("go", "Get", vec![], true),
                    plain_lang("rust", "get", vec![], false),
                ],
            }],
        }],
        externs: vec![ExternDecl {
            name: "open".into(),
            params: vec![ExternParam {
                variadic: false,
                name: "seed".into(),
                r#type: string_t(),
            }],
            r#return: reference("keepkit#locker"),
            langs: vec![
                plain_lang(
                    "go",
                    "OpenStore",
                    vec![CallArg::Param("seed".into())],
                    false,
                ),
                plain_lang(
                    "rust",
                    "open_vault",
                    vec![CallArg::Param("seed".into())],
                    false,
                ),
            ],
        }],
    }
}

/// `lib` with every binding but `lang`'s dropped, mirroring how
/// `handle_source` scopes its vectors to the one target under test.
fn only_lang(mut lib: ExtLib, lang: &str) -> ExtLib {
    lib.langs.retain(|l| l.lang == lang);
    for handle in &mut lib.types {
        if let Some(instance) = &mut handle.instance {
            instance.names.retain(|n| n.lang == lang);
        }
        for method in &mut handle.methods {
            method.langs.retain(|l| l.lang == lang);
        }
    }
    for decl in &mut lib.externs {
        decl.langs.retain(|l| l.lang == lang);
    }
    lib
}

/// The module: a `client` entry constructing the handle from an argument and
/// one op whose own `impl` body reads it back.
pub fn per_lang_handle_module(lang: &str) -> Module {
    let seed = EntryField {
        name: "seed".into(),
        target: string_t(),
        sources: vec![Source::Arg],
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    };
    let locker = EntryField {
        name: "locker".into(),
        target: reference("keepkit#locker"),
        sources: vec![],
        format: None,
        transforms: vec![],
        select: None,
        call: Some(EntryCall {
            ns: "keepkit".into(),
            func: "open".into(),
            args: vec![CallArg::Ref(vec!["seed".into()])],
        }),
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    };
    let value = Shape {
        id: "sk#client.value".into(),
        kind: ShapeKind::Operation {
            input: None,
            input_name: None,
            output: Some(string_t()),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["locker".into()],
                method: "get".into(),
                args: vec![],
            }),
        },
        traits: vec![],
    };
    let entry = Shape {
        id: "sk#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![seed, locker],
            operations: vec![value],
        },
        traits: vec![Trait {
            id: "pub".into(),
            value: serde_json::Value::Null,
        }],
    };
    Module {
        name: "sk".into(),
        shapes: vec![entry],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![only_lang(keepkit(), lang)],
        tests: vec![],
    }
}

/// [`per_lang_handle_module`], wrapped in a `Model`.
pub fn per_lang_handle_model(lang: &str) -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![per_lang_handle_module(lang)],
    }
}

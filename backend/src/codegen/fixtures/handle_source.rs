//! A worked example of a field sourced from a foreign handle's method: a
//! settings provider constructed once (`provider = envkit.new_provider(..)`),
//! read once into a config value (`config: cfg = .provider.get()`) that
//! several `@http` operations then consume through their `endpoint:`, an
//! injectable second read with an argument (`scoped: cfg @with =
//! .provider.get_for(.region)`), and an op whose own `impl` body reads the
//! same handle (`probe`), proving that reading a method borrows the handle
//! and coexists with every other reader.
//!
//! Every extern is spelled for all three targets, and [`handle_source_module`]
//! keeps the one language a caller asks for (an extern bound for more than
//! one language must be covered by a declared test, which is not what these
//! vectors are about), so one definition serves the Go, TypeScript and Rust
//! compiled-vector checks (`*_ext_roundtrip.rs`, each compiling the
//! generated SDK against the `envkit` stand-in library under its own
//! `codegen-tests/*-ext/fixtures/`) and each target's unit tests, instead
//! of three near-identical copies drifting apart. Not `cfg(test)`: the
//! integration tests link it from outside the crate.

use std::collections::BTreeMap;

use crate::ir::{
    CallArg, EntryCall, EntryField, ExtLib, ExternDecl, ExternLang, ExternParam, ForeignField,
    ForeignStruct, LangPath, Member, Model, Module, OpImplCall, OpaqueType, Prim, ReturnsField,
    ReturnsLit, ReturnsValue, Shape, ShapeKind, Source, TemplatePart, Trait, Tref, WireBinding,
    WireValue, YieldsPos, TONO_IR_VERSION,
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

fn member(name: &str) -> Member {
    Member {
        name: name.into(),
        target: string_t(),
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn structure(id: &str, members: &[&str]) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members: members.iter().map(|m| member(m)).collect(),
        },
        traits: vec![],
    }
}

fn foreign_struct(name: &str, fields: &[&str]) -> ForeignStruct {
    ForeignStruct {
        name: name.into(),
        fields: fields
            .iter()
            .map(|f| ForeignField {
                name: (*f).into(),
                r#type: string_t(),
            })
            .collect(),
    }
}

fn field(name: &str, target: Tref, sources: Vec<Source>) -> EntryField {
    EntryField {
        name: name.into(),
        target,
        sources,
        format: None,
        transforms: vec![],
        select: None,
        call: None,
        handle_call: None,
        binds: vec![],
        constraints: vec![],
        traits: vec![],
    }
}

/// One language binding of a `cfg`-returning method: the raw foreign
/// struct's own two fields projected onto the logical `cfg`, spelled in
/// that language's own field casing (`read_field`/`write_field`).
#[allow(clippy::too_many_arguments)]
fn cfg_lang(
    lang: &str,
    symbol: &str,
    call_args: Vec<CallArg>,
    raw: &str,
    read_field: &str,
    write_field: &str,
    ctx: bool,
) -> ExternLang {
    ExternLang {
        lang: lang.into(),
        symbol: symbol.into(),
        call_args,
        yields: vec![YieldsPos {
            name: "c".into(),
            r#type: Some(reference(&format!("kvs#{raw}"))),
            is_error: false,
        }],
        returns: Some(ReturnsLit {
            r#type: reference("kvs#cfg"),
            fields: vec![
                ReturnsField {
                    name: "endpoint_read".into(),
                    value: ReturnsValue::Field(vec!["c".into(), read_field.into()]),
                },
                ReturnsField {
                    name: "endpoint_write".into(),
                    value: ReturnsValue::Field(vec!["c".into(), write_field.into()]),
                },
            ],
        }),
        errors: vec![],
        sync: false,
        infallible: false,
        ctx,
    }
}

/// A `cfg`-returning handle method bound for all three targets. `ctx` marks
/// the Go binding as taking a context (only meaningful there).
fn cfg_method(
    name: &str,
    params: Vec<ExternParam>,
    call_args: Vec<CallArg>,
    ctx: bool,
) -> ExternDecl {
    let pascal = |s: &str| {
        s.split('_')
            .map(|p| {
                let mut c = p.chars();
                match c.next() {
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect::<String>()
    };
    let camel = |s: &str| {
        let p = pascal(s);
        let mut c = p.chars();
        match c.next() {
            Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    };
    ExternDecl {
        name: name.into(),
        params,
        r#return: reference("kvs#cfg"),
        langs: vec![
            cfg_lang(
                "go",
                &pascal(name),
                call_args.clone(),
                "go_cfg",
                "ReadURL",
                "WriteURL",
                ctx,
            ),
            cfg_lang(
                "ts",
                &camel(name),
                call_args.clone(),
                "ts_cfg",
                "readUrl",
                "writeUrl",
                false,
            ),
            cfg_lang(
                "rust",
                name,
                call_args,
                "rs_cfg",
                "read_url",
                "write_url",
                false,
            ),
        ],
    }
}

fn plain_lang(lang: &str, symbol: &str, call_args: Vec<CallArg>) -> ExternLang {
    ExternLang {
        lang: lang.into(),
        symbol: symbol.into(),
        call_args,
        yields: vec![],
        returns: None,
        errors: vec![],
        sync: false,
        infallible: false,
        ctx: false,
    }
}

/// The `envkit` library: a `provider` handle with two `cfg`-returning
/// methods (one taking a Go context and no arguments, one taking a region)
/// and a free constructor.
fn envkit() -> ExtLib {
    let name_arg = vec![CallArg::Param("name".into())];
    let region_arg = vec![CallArg::Param("region".into())];
    ExtLib {
        name: "envkit".into(),
        langs: vec![
            LangPath {
                lang: "go".into(),
                path: "tono-ext-fixture/envkit".into(),
            },
            LangPath {
                lang: "ts".into(),
                path: "@company/envkit".into(),
            },
            LangPath {
                lang: "rust".into(),
                path: "envkit".into(),
            },
        ],
        structs: vec![
            foreign_struct("go_cfg", &["ReadURL", "WriteURL"]),
            foreign_struct("ts_cfg", &["readUrl", "writeUrl"]),
            foreign_struct("rs_cfg", &["read_url", "write_url"]),
        ],
        types: vec![OpaqueType {
            name: "provider".into(),
            instance: None,
            methods: vec![
                cfg_method("get", vec![], vec![], true),
                cfg_method(
                    "get_for",
                    vec![ExternParam {
                        name: "region".into(),
                        r#type: string_t(),
                    }],
                    region_arg,
                    false,
                ),
            ],
        }],
        externs: vec![ExternDecl {
            name: "new_provider".into(),
            params: vec![ExternParam {
                name: "name".into(),
                r#type: string_t(),
            }],
            r#return: reference("envkit#provider"),
            langs: vec![
                plain_lang("go", "NewProvider", name_arg.clone()),
                plain_lang("ts", "newProvider", name_arg.clone()),
                plain_lang("rust", "new_provider", name_arg),
            ],
        }],
    }
}

fn http_op(id: &str, method: &str, path: Vec<TemplatePart>, endpoint_field: &str) -> Shape {
    let input_name = "i";
    let input = if method == "GET" {
        "kvs#item_ref"
    } else {
        "kvs#item"
    };
    Shape {
        id: id.into(),
        kind: ShapeKind::Operation {
            input: Some(reference(input)),
            input_name: Some(input_name.into()),
            output: Some(reference("kvs#item")),
            errors: vec![],
            wire: Some(Box::new(WireBinding {
                method: method.into(),
                uri: WireValue::Template(path),
                body: (method == "PUT").then(|| WireValue::Param(vec![])),
                response_bindings: BTreeMap::new(),
                success: vec![],
                endpoint: Some(WireValue::Field(vec![
                    "config".into(),
                    endpoint_field.into(),
                ])),
                request_headers: vec![],
                query: vec![],
                timeout: None,
                retry: None,
            })),
            impl_call: None,
        },
        traits: vec![],
    }
}

/// The module: `envkit` (bound for `lang` only: `"go"`, `"ts"` or
/// `"rust"`), the logical shapes, and the `client` entry.
pub fn handle_source_module(lang: &str) -> Module {
    let provider = {
        let mut f = field("provider", reference("envkit#provider"), vec![]);
        f.call = Some(EntryCall {
            ns: "envkit".into(),
            func: "new_provider".into(),
            args: vec![CallArg::Lit(serde_json::Value::String("kvs".into()))],
        });
        f
    };
    let config = {
        let mut f = field("config", reference("kvs#cfg"), vec![]);
        f.handle_call = Some(OpImplCall {
            recv: vec!["provider".into()],
            method: "get".into(),
            args: vec![],
        });
        f
    };
    let scoped = {
        let mut f = field("scoped", reference("kvs#cfg"), vec![Source::With]);
        f.handle_call = Some(OpImplCall {
            recv: vec!["provider".into()],
            method: "get_for".into(),
            args: vec![CallArg::Ref(vec!["region".into()])],
        });
        f
    };
    let read = http_op(
        "kvs#client.read",
        "GET",
        vec![
            TemplatePart::Lit("/items/".into()),
            TemplatePart::Param(vec!["id".into()]),
        ],
        "endpoint_read",
    );
    let write = http_op(
        "kvs#client.write",
        "PUT",
        vec![TemplatePart::Lit("/items".into())],
        "endpoint_write",
    );
    let probe = Shape {
        id: "kvs#client.probe".into(),
        kind: ShapeKind::Operation {
            input: None,
            input_name: None,
            output: Some(reference("kvs#cfg")),
            errors: vec![],
            wire: None,
            impl_call: Some(OpImplCall {
                recv: vec!["provider".into()],
                method: "get_for".into(),
                args: vec![CallArg::Ref(vec!["region".into()])],
            }),
        },
        traits: vec![],
    };
    let entry = Shape {
        id: "kvs#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![
                field("region", string_t(), vec![Source::Arg]),
                provider,
                config,
                scoped,
            ],
            operations: vec![read, write, probe],
        },
        traits: vec![Trait {
            id: "pub".into(),
            value: serde_json::Value::Null,
        }],
    };
    Module {
        name: "kvs".into(),
        shapes: vec![
            structure("kvs#cfg", &["endpoint_read", "endpoint_write"]),
            structure("kvs#item", &["id", "body"]),
            structure("kvs#item_ref", &["id"]),
            entry,
        ],
        operations: vec![],
        extensions: vec![],
        ext_libs: vec![only_lang(envkit(), lang)],
        tests: vec![],
    }
}

/// `lib` with every binding but `lang`'s dropped (module path, handle
/// methods, free externs alike).
fn only_lang(mut lib: ExtLib, lang: &str) -> ExtLib {
    lib.langs.retain(|l| l.lang == lang);
    for handle in &mut lib.types {
        for method in &mut handle.methods {
            method.langs.retain(|l| l.lang == lang);
        }
    }
    for decl in &mut lib.externs {
        decl.langs.retain(|l| l.lang == lang);
    }
    lib
}

/// [`handle_source_module`], wrapped in a `Model`.
pub fn handle_source_model(lang: &str) -> Model {
    Model {
        tono_ir_version: TONO_IR_VERSION,
        modules: vec![handle_source_module(lang)],
    }
}

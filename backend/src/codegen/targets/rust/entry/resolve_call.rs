//! The extern-call leaf: `field: T = ns.fn(args)` lowered to a Rust call
//! against the declared crate, its `yields`/`returns` projection, and its
//! `errors:` sentinel mapping. Split out from `resolve.rs` (a new leaf
//! entirely, not a moved one) to keep that file's leaf table from growing
//! past the file-size gate.
//!
//! `ns`/`fn` are guaranteed to resolve against a real `extern` carrying a
//! `lang: "rust"` block: `validate::call_resolves` checks this ahead of
//! every Rust generation call, so the lookups below are `expect`ed rather
//! than diagnosed here — a miss at this point is a validator gap, not an
//! authoring error.
//!
//! The call is always awaited: Rust is an async-lowering target, and an
//! arbitrary third-party symbol is exactly the "might block on I/O" case
//! that target already treats as async. `constructor.rs` gates the
//! surrounding `new`/`build` to `async fn` whenever an entry has one of
//! these fields.

use std::fmt::Write as _;

use super::resolve::Resolver;
use super::*;
use crate::codegen::entries::plan::Emitter;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, ErrorBinding, ExtLib, ExternDecl, ExternLang,
    ReturnsField, ReturnsLit, ReturnsValue, Select, YieldsPos,
};

fn find_lib<'a>(module: &'a Module, ns: &str) -> &'a ExtLib {
    module
        .ext_libs
        .iter()
        .find(|l| l.name == ns)
        .expect("validate::call_resolves checked this ext block exists")
}

fn find_extern<'a>(lib: &'a ExtLib, func: &str) -> &'a ExternDecl {
    lib.externs
        .iter()
        .find(|e| e.name == func)
        .expect("validate::call_resolves checked this extern exists")
}

fn find_rust_lang(decl: &ExternDecl) -> &ExternLang {
    decl.langs
        .iter()
        .find(|l| l.lang == "rust")
        .expect("validate::call_resolves checked a rust block exists")
}

/// A `@default`-shaped best-effort Rust literal for a raw JSON call
/// argument: exact typed rendering needs the logical parameter's declared
/// type positionally matched against `call_args`, which is not always
/// possible (a `Ctor` field's `Lit` has no positional `ExternParam` at all),
/// so this stays intentionally naive rather than half-implementing the
/// general `literal()` machinery for a shape it cannot always see.
fn json_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => format!("{s:?}.to_string()"),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Null => "Default::default()".to_string(),
        _ => format!("serde_json::json!({v})"),
    }
}

/// One `CallArg` as a Rust expression, in the target's own call syntax.
fn call_arg_expr(r: &mut Resolver<'_, '_>, lib: &ExtLib, arg: &CallArg) -> String {
    match arg {
        // The DAG resolves every real argument to a `Ref` path into the
        // entry before codegen ever sees it; a bare `Param` only arises for
        // a logical parameter no `call_args` position renamed away, so this
        // reads the same-named field as a best effort.
        CallArg::Param(name) => format!("{}{}.clone()", r.arg_prefix, field_snake(name, r.config)),
        // Cloned, not moved: `s` (the composed settings struct) is still
        // read later (the frozen `ClientOptions`, `Client { settings: s,
        // .. }`), the same "copy, don't move a sibling" rule every other
        // leaf reading a resolved field already follows.
        CallArg::Ref(path) => format!("({}).clone()", r.path_expr(path)),
        CallArg::Lit(v) => json_literal(v),
        CallArg::List(items) => {
            let rendered: Vec<String> = items.iter().map(|a| call_arg_expr(r, lib, a)).collect();
            format!("vec![{}]", rendered.join(", "))
        }
        CallArg::Ctor(CallCtor { name, fields }) => {
            lib.structs
                .iter()
                .find(|s| &s.name == name)
                .expect("checker rejects a ctor naming an undeclared foreign struct");
            let rendered: Vec<String> = fields
                .iter()
                .map(|(field_name, value)| {
                    format!("{field_name}: {}", call_arg_expr(r, lib, value))
                })
                .collect();
            format!("{name} {{ {} }}", rendered.join(", "))
        }
        CallArg::Call(nested) => call_expr(r, nested),
    }
}

/// The bare call expression (crate-qualified symbol, args, `.await`), reused
/// both as a top-level field's assignment source and as a nested argument's
/// own value.
fn call_expr(r: &mut Resolver<'_, '_>, call: &EntryCall) -> String {
    let lib = find_lib(r.module, &call.ns);
    let decl = find_extern(lib, &call.func);
    let lang = find_rust_lang(decl);
    // The crate is referenced fully qualified at the call site rather than
    // through a separate `use`: an external crate name is already in scope
    // by declaration (no glob/ambiguity risk), and this sidesteps having to
    // invent a `Symbol` shape for a dependency that lives outside the
    // generated SDK's own module tree.
    let crate_ident = lib
        .langs
        .iter()
        .find(|l| l.lang == "rust")
        .map(|l| l.path.replace('-', "_"))
        .expect("validate::call_resolves checked a rust module path exists");
    let args: Vec<String> = lang
        .call_args
        .iter()
        .map(|a| call_arg_expr(r, lib, a))
        .collect();
    format!("{crate_ident}::{}({}).await", lang.symbol, args.join(", "))
}

/// A `returns:` field's source expression, read off the `yields` binding
/// named in `ReturnsValue::Field`'s head segment (the remaining segments are
/// the foreign shape's own field names, verbatim: no casing engine touches a
/// declared-foreign form).
fn returns_value_expr(value: &ReturnsValue) -> String {
    match value {
        ReturnsValue::Field(path) => path.join("."),
        ReturnsValue::Select(select) => select_expr(select),
    }
}

/// A `match .subject { ... }` selection inside a `returns:` field, spelled
/// as a standalone expression (not the shared `Stmt` tree the rest of the
/// plan renders through, since this text is composed directly): the same
/// Display-string comparison the plan's own switch leaves use, so one form
/// covers a string, a numeric, or an open-enum subject alike.
fn select_expr(select: &Select) -> String {
    let subject = select.subject.join(".");
    let mut arms = String::new();
    for arm in &select.arms {
        let value = match &arm.value {
            ArmValue::Field(path) => path.join("."),
            ArmValue::Lit(v) => json_literal(v),
            // A nested source stack inside a returns-projection match arm
            // has no field/env context to resolve against here (this text
            // is composed outside the shared resolution plan); falling back
            // to the subject itself keeps the arm total without inventing a
            // resolution the grammar does not give this leaf enough to spell.
            ArmValue::Sources(_) => subject.clone(),
        };
        match &arm.pattern {
            // A match arm's pattern position takes a bare literal, not an
            // expression: `json_literal`'s `"x".to_string()` (built for a
            // value position) is not valid pattern syntax, so this uses the
            // same bare-literal spelling `pattern_literal` already gives the
            // shared plan's own switch leaves.
            Some(pat) => {
                let _ = write!(arms, "{} => {value}, ", pattern_literal(pat));
            }
            None => {
                let _ = write!(arms, "_ => {value}, ");
            }
        }
    }
    if select.arms.iter().all(|a| a.pattern.is_some()) {
        let _ = write!(arms, "_ => {subject}.clone(), ");
    }
    format!("match ({subject}).to_string().as_str() {{ {arms}}}")
}

/// The `errors:` mapping inside the `Err(e)` arm: a declared sentinel names
/// a diagnosed `ConfigError`; anything else is a `ContractError` naming the
/// extern, the same "declaration is a hypothesis the target build already
/// enforces, so an unmapped failure names its origin" idiom `impl_op` uses
/// for a bespoke operation.
fn error_match(ns: &str, func: &str, errors: &[ErrorBinding]) -> String {
    let mut arms = String::new();
    for e in errors {
        let _ = write!(
            arms,
            "{:?} => TonoError::Config(ConfigError {{ message: e.to_string() }}), ",
            e.sentinel,
        );
    }
    let contract_name = format!("{ns}.{func}");
    format!(
        "match e.to_string().as_str() {{ {arms}_ => TonoError::Contract(ContractError {{ contract_name: {contract_name:?}.to_string(), cause: e.to_string().into() }}), }}"
    )
}

/// `yields`' non-error positions bound out of `Ok(..)`: a single position
/// binds bare, more than one destructures a tuple (the native `Result` only
/// carries one success value, so multiple `yields` names are always a tuple
/// of it).
fn ok_pattern(positions: &[&YieldsPos]) -> String {
    match positions {
        [one] => one.name.clone(),
        many => format!(
            "({})",
            many.iter()
                .map(|p| p.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// The `dest = ...` (or field-by-field) assignment from the `Ok(..)` binding
/// into `dest`: a declared `returns:` projects fields one at a time, its
/// absence assigns the whole (possibly cast) binding.
fn ok_assign(dest: &str, ok_binding: &str, returns: Option<&ReturnsLit>) -> String {
    let Some(returns) = returns else {
        return format!("{dest} = {ok_binding};");
    };
    let fields: Vec<String> = returns
        .fields
        .iter()
        .map(|ReturnsField { name, value }| format!("{name}: {}", returns_value_expr(value)))
        .collect();
    let ty = type_ident_from_id(&match &returns.r#type {
        Tref::Ref { id, .. } => id.clone(),
        // A returns: type is always a declared named shape (the checker
        // enforces this); a primitive/list/map target has nothing to
        // project fields into and never reaches this branch.
        _ => String::new(),
    });
    format!("{dest} = {ty} {{ {} }};", fields.join(", "))
}

/// The extern-call assignment: resolves `ns.fn` against the
/// module's own `ext_libs`, emits the awaited call in the declared argument
/// order, destructures `yields`, projects `returns:`, and maps `errors:` —
/// or, absent `yields`, treats the call as a plain `Result<T, E>` whose `Ok`
/// assigns straight into `dest`.
pub(super) fn call_assign(
    r: &mut Resolver<'_, '_>,
    _field: &EntryField,
    call: &EntryCall,
    dest: &str,
) -> String {
    let lib = find_lib(r.module, &call.ns);
    let decl = find_extern(lib, &call.func);
    let lang = find_rust_lang(decl);
    let errors = lang.errors.clone();
    let returns = lang.returns.clone();
    let ns = call.ns.clone();
    let func = call.func.clone();
    let yields = lang.yields.clone();
    let expr = call_expr(r, call);

    let ok_positions: Vec<&YieldsPos> = yields.iter().filter(|y| !y.is_error).collect();

    let ok_pat = if yields.is_empty() {
        "v".to_string()
    } else {
        ok_pattern(&ok_positions)
    };
    format!(
        "match {expr} {{\n    Ok({ok_pat}) => {{ {assign} }}\n    Err(e) => {{ return Err({mapped}); }}\n}}",
        assign = ok_assign(dest, &ok_pat, returns.as_ref()),
        mapped = error_match(&ns, &func, &errors),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::rust::rust_casing;
    use crate::codegen::test_support::bare_entry_field;
    use crate::ir::{
        CallArg, EntryCall, ExtLib, ExternDecl, ExternParam, LangPath, ReturnsField, ReturnsLit,
        ReturnsValue, SelectArm, YieldsPos,
    };

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

    /// A plain call field (no `yields`, no `returns`) against a lib that
    /// declares no `errors:` mapping: `call_assign` must not panic, must
    /// await the crate-qualified call, and must fall back to `Ok(v)`
    /// binding straight into `dest`.
    #[test]
    fn a_plain_call_field_emits_an_awaited_call_with_no_projection() {
        let region = bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::Arg]);
        let mut config = bare_entry_field("config", Tref::Prim(Prim::String), vec![]);
        config.call = Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![CallArg::Ref(vec!["region".into()])],
        });
        let mut module = module_of(vec![client_shape(vec![config, region])]);
        module.ext_libs = vec![ExtLib {
            name: "companyconfig".into(),
            langs: vec![LangPath {
                lang: "rust".into(),
                path: "company-config".into(),
            }],
            structs: vec![],
            types: vec![],
            externs: vec![ExternDecl {
                name: "load".into(),
                params: vec![ExternParam {
                    name: "region".into(),
                    r#type: Tref::Prim(Prim::String),
                }],
                r#return: Tref::Prim(Prim::String),
                langs: vec![ExternLang {
                    lang: "rust".into(),
                    symbol: "Client::load".into(),
                    call_args: vec![CallArg::Ref(vec!["region".into()])],
                    yields: vec![],
                    returns: None,
                    errors: vec![],
                }],
            }],
        }];
        let out = entry_text(&module, &rust_casing());
        assert!(out.contains("company_config::Client::load"), "{out}");
        assert!(out.contains(".await"), "{out}");
        assert!(out.contains("async fn new"), "{out}");
    }

    /// A `@with` field backed by a call fallback builds through
    /// `ClientBuilder`, and the injected value wins over the call: this
    /// exercises `with_present_cond`/`with_assign` (the plain boolean
    /// `if`/`else` the shared plan wraps a call in when `@with` is also
    /// declared).
    #[test]
    fn a_with_field_backed_by_a_call_fallback_prefers_the_injected_value() {
        let mut bus = bare_entry_field("bus", Tref::Prim(Prim::String), vec![Source::With]);
        bus.call = Some(EntryCall {
            ns: "companybus".into(),
            func: "connect".into(),
            args: vec![],
        });
        let module_with_lib = |mut module: Module| {
            module.ext_libs = vec![ExtLib {
                name: "companybus".into(),
                langs: vec![LangPath {
                    lang: "rust".into(),
                    path: "company_bus".into(),
                }],
                structs: vec![],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "connect".into(),
                    params: vec![],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![ExternLang {
                        lang: "rust".into(),
                        symbol: "connect".into(),
                        call_args: vec![],
                        yields: vec![],
                        returns: None,
                        errors: vec![],
                    }],
                }],
            }];
            module
        };
        let module = module_with_lib(module_of(vec![client_shape(vec![bus])]));
        let out = entry_text(&module, &rust_casing());
        assert!(out.contains(".is_some()"), "{out}");
        assert!(out.contains(".unwrap();"), "{out}");
        assert!(out.contains("company_bus::connect()"), "{out}");
    }

    /// A call declaring `yields`/`returns`/`errors:` projects the success
    /// binding into the declared struct fields and maps the declared
    /// sentinel, without panicking.
    #[test]
    fn a_call_field_with_yields_returns_and_errors_projects_and_maps() {
        let mut conn = bare_entry_field(
            "conn",
            Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            vec![],
        );
        conn.call = Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![],
        });
        let app_config = Shape {
            id: "m#app_config".into(),
            kind: ShapeKind::Structure {
                params: vec![],
                members: vec![],
            },
            traits: vec![],
        };
        let mut module = module_of(vec![client_shape(vec![conn]), app_config]);
        module.ext_libs = vec![ExtLib {
            name: "companyconfig".into(),
            langs: vec![LangPath {
                lang: "rust".into(),
                path: "company_config".into(),
            }],
            structs: vec![],
            types: vec![],
            externs: vec![ExternDecl {
                name: "load".into(),
                params: vec![],
                r#return: Tref::Ref {
                    id: "m#app_config".into(),
                    args: vec![],
                },
                langs: vec![ExternLang {
                    lang: "rust".into(),
                    symbol: "Client::load".into(),
                    call_args: vec![],
                    yields: vec![
                        YieldsPos {
                            name: "cfg".into(),
                            r#type: Some(Tref::Prim(Prim::String)),
                            is_error: false,
                        },
                        YieldsPos {
                            name: "err".into(),
                            r#type: None,
                            is_error: true,
                        },
                    ],
                    returns: Some(ReturnsLit {
                        r#type: Tref::Ref {
                            id: "m#app_config".into(),
                            args: vec![],
                        },
                        fields: vec![ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Field(vec!["cfg".into(), "Host".into()]),
                        }],
                    }),
                    errors: vec![crate::ir::ErrorBinding {
                        sentinel: "ErrBusy".into(),
                        r#type: "overloaded".into(),
                    }],
                }],
            }],
        }];
        let out = entry_text(&module, &rust_casing());
        assert!(out.contains("Ok(cfg)"), "{out}");
        assert!(out.contains("endpoint: cfg.Host"), "{out}");
        assert!(out.contains("\"ErrBusy\""), "{out}");
        assert!(out.contains("ContractError"), "{out}");
    }

    #[test]
    fn json_literal_covers_every_json_value_kind() {
        assert_eq!(json_literal(&serde_json::json!("hi")), "\"hi\".to_string()");
        assert_eq!(json_literal(&serde_json::json!(true)), "true");
        assert_eq!(json_literal(&serde_json::json!(3)), "3");
        assert_eq!(json_literal(&serde_json::json!(null)), "Default::default()");
        assert_eq!(
            json_literal(&serde_json::json!([1, 2])),
            "serde_json::json!([1,2])"
        );
    }

    #[test]
    fn ok_pattern_destructures_a_tuple_for_more_than_one_position() {
        let a = YieldsPos {
            name: "a".into(),
            r#type: None,
            is_error: false,
        };
        let b = YieldsPos {
            name: "b".into(),
            r#type: None,
            is_error: false,
        };
        assert_eq!(ok_pattern(&[&a]), "a");
        assert_eq!(ok_pattern(&[&a, &b]), "(a, b)");
    }

    #[test]
    fn select_expr_covers_a_field_arm_a_sources_arm_and_the_synthesized_default() {
        let select = Select {
            subject: vec!["cfg".into(), "env".into()],
            arms: vec![
                SelectArm {
                    pattern: Some(serde_json::json!("prod")),
                    value: ArmValue::Field(vec!["cfg".into(), "host".into()]),
                },
                SelectArm {
                    pattern: Some(serde_json::json!("dev")),
                    value: ArmValue::Sources(vec![]),
                },
            ],
        };
        let out = select_expr(&select);
        assert!(out.contains("\"prod\" => cfg.host"), "{out}");
        // Every declared arm carries a pattern, so the function synthesizes
        // a trailing wildcard arm rather than leaving the match non-total.
        assert!(out.contains("_ => cfg.env.clone()"), "{out}");
    }

    #[test]
    fn error_match_maps_every_declared_sentinel_and_falls_back_to_contract_error() {
        let out = error_match(
            "companybus",
            "send",
            &[
                crate::ir::ErrorBinding {
                    sentinel: "ErrBusy".into(),
                    r#type: "overloaded".into(),
                },
                crate::ir::ErrorBinding {
                    sentinel: "ErrGone".into(),
                    r#type: "not_found".into(),
                },
            ],
        );
        assert!(out.contains("\"ErrBusy\" =>"), "{out}");
        assert!(out.contains("\"ErrGone\" =>"), "{out}");
        assert!(out.contains("companybus.send"), "{out}");
        assert!(out.contains("ContractError"), "{out}");
    }

    /// A call whose `call_args` mix every `CallArg` variant (`Param`,
    /// `List`, `Ctor`, a nested `Call`), and whose `yields` carries two
    /// non-error positions, exercises `call_arg_expr`'s full match and the
    /// `ok_pattern` tuple branch together.
    #[test]
    fn a_call_field_with_every_call_arg_variant_and_a_nested_call_emits_without_panicking() {
        let mut region = bare_entry_field("region", Tref::Prim(Prim::String), vec![Source::Arg]);
        region.name = "region".into();
        let mut token = bare_entry_field("token", Tref::Prim(Prim::String), vec![]);
        token.call = Some(EntryCall {
            ns: "companyauth".into(),
            func: "sign".into(),
            args: vec![],
        });
        let mut config = bare_entry_field("config", Tref::Prim(Prim::String), vec![]);
        config.call = Some(EntryCall {
            ns: "companyconfig".into(),
            func: "load".into(),
            args: vec![
                CallArg::Param("region".into()),
                CallArg::List(vec![CallArg::Lit(serde_json::json!(1))]),
                CallArg::Ctor(crate::ir::CallCtor {
                    name: "opts".into(),
                    fields: [("retries".to_string(), CallArg::Lit(serde_json::json!(3)))]
                        .into_iter()
                        .collect(),
                }),
                CallArg::Call(Box::new(EntryCall {
                    ns: "companyauth".into(),
                    func: "sign".into(),
                    args: vec![],
                })),
            ],
        });
        let mut module = module_of(vec![client_shape(vec![token, config, region])]);
        module.ext_libs = vec![
            ExtLib {
                name: "companyauth".into(),
                langs: vec![LangPath {
                    lang: "rust".into(),
                    path: "company-auth".into(),
                }],
                structs: vec![],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "sign".into(),
                    params: vec![],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![ExternLang {
                        lang: "rust".into(),
                        symbol: "sign".into(),
                        call_args: vec![],
                        yields: vec![],
                        returns: None,
                        errors: vec![],
                    }],
                }],
            },
            ExtLib {
                name: "companyconfig".into(),
                langs: vec![LangPath {
                    lang: "rust".into(),
                    path: "company_config".into(),
                }],
                structs: vec![crate::ir::ForeignStruct {
                    name: "opts".into(),
                    fields: vec![crate::ir::ForeignField {
                        name: "retries".into(),
                        r#type: Tref::Prim(Prim::I32),
                    }],
                }],
                types: vec![],
                externs: vec![ExternDecl {
                    name: "load".into(),
                    params: vec![],
                    r#return: Tref::Prim(Prim::String),
                    langs: vec![ExternLang {
                        lang: "rust".into(),
                        symbol: "Client::load".into(),
                        call_args: vec![
                            CallArg::Param("region".into()),
                            CallArg::List(vec![CallArg::Lit(serde_json::json!(1))]),
                            CallArg::Ctor(crate::ir::CallCtor {
                                name: "opts".into(),
                                fields: [(
                                    "retries".to_string(),
                                    CallArg::Lit(serde_json::json!(3)),
                                )]
                                .into_iter()
                                .collect(),
                            }),
                            CallArg::Call(Box::new(EntryCall {
                                ns: "companyauth".into(),
                                func: "sign".into(),
                                args: vec![],
                            })),
                        ],
                        yields: vec![
                            YieldsPos {
                                name: "a".into(),
                                r#type: Some(Tref::Prim(Prim::String)),
                                is_error: false,
                            },
                            YieldsPos {
                                name: "b".into(),
                                r#type: Some(Tref::Prim(Prim::String)),
                                is_error: false,
                            },
                        ],
                        returns: None,
                        errors: vec![],
                    }],
                }],
            },
        ];
        let out = entry_text(&module, &rust_casing());
        assert!(out.contains("vec![1]"), "{out}");
        assert!(out.contains("opts { retries: 3 }"), "{out}");
        assert!(out.contains("company_auth::sign().await"), "{out}");
        assert!(out.contains("Ok((a, b))"), "{out}");
    }
}

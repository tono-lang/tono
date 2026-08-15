//! The `= ns.fn(args)` extern-call field source: importing the foreign
//! symbol, calling it with the `ts` language block's own argument order
//! (including a `Ctor` struct literal, foreign field names verbatim),
//! projecting `yields`/`returns` onto the declared logical type, and mapping
//! a declared sentinel (or any unmapped failure) onto a typed error at the
//! `ContractError` boundary already used elsewhere in this target.
//!
//! TypeScript's own identity is "throws, and may return a Promise": nothing
//! in the IR marks a given extern call sync or async, and the compiler
//! cannot know statically whether a third-party function returns a Promise.
//! So every call is awaited unconditionally here (`await` on a plain value
//! is a safe no-op); `class_decl` (in `entry/mod.rs`) is what turns an entry
//! with at least one such field into an async-constructed client (a
//! `static async create`, not a plain `constructor`).
//!
//! Scope: a free function in an `ext` block (`module.ext_libs[].externs`),
//! including one whose logical return type is an opaque handle (the
//! `companybus.connect(..)` shape: no `yields`, the raw call result already
//! is the logical value). `entries::validate_entries` guarantees, before
//! this runs, that a call names a declared extern carrying a `ts` binding,
//! that a projecting `yields` names a `returns`, and that a `ts` binding
//! names at most one non-error `yields` position (a single call result has
//! nothing to bind a second one to); the lookups below still fail loudly on
//! a broken invariant rather than silently miscompiling. Not yet supported,
//! left as a clear generation-time panic rather than silently wrong output:
//! `CallArg::Call` (a nested extern call used as another call's argument).
//! An opaque handle's own methods (`type publisher { extern send(..) }`,
//! invoked from an op's `impl`) are a different call site that no codegen
//! consumes yet.

use super::{field_camel, module_symbol, pascal, Resolver};
use crate::codegen::entries::plan::Emitter;
use crate::codegen::ops::error_names;
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::Decl;
use crate::ir::{
    ArmValue, CallArg, CallCtor, EntryCall, EntryField, ExternParam, Module, ReturnsValue, Select,
};

/// The typed-error class name a declared sentinel maps to: the bare
/// identifier the `.tono` author wrote (`overloaded`), cased through the
/// same engine as every other generated identifier, suffixed the way every
/// other category in the closed taxonomy is (`ContractError`,
/// `ConfigError`, ...).
pub(super) fn sentinel_error_class(sentinel_type: &str) -> String {
    format!("{}Error", pascal(sentinel_type))
}

/// The generated class for one distinct sentinel-mapped type name, emitted
/// once per module regardless of how many extern calls declare it. Rooted
/// under the same taxonomy root as every other category, so the existing
/// `instanceof TonoError` boundary checks still see it.
pub(super) fn sentinel_error_decl(sentinel_type: &str, module: &Module) -> Decl {
    let name = sentinel_error_class(sentinel_type);
    let root = error_names().root;
    Decl::raw_with(
        format!(
            "// {name} is the typed error a declared ext sentinel maps to; a\n\
         // third-party call that throws this sentinel surfaces as this class\n\
         // instead of the generic ContractError fallback.\n\
         export class {name} extends {root} {{\n\
         \x20 constructor(readonly cause: unknown) {{\n\
         \x20   super({sentinel_type:?});\n\
         \x20   this.name = {name:?};\n\
         \x20 }}\n\
         }}",
        ),
        vec![module_symbol(&root, module)],
    )
}

fn json_literal(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => format!("{s:?}"),
        serde_json::Value::Array(items) => {
            format!(
                "[{}]",
                items
                    .iter()
                    .map(json_literal)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
        serde_json::Value::Object(map) => format!(
            "{{ {} }}",
            map.iter()
                .map(|(k, v)| format!("{k}: {}", json_literal(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Render one node of a `ts` language block's `call_args` template. `Param`
/// substitutes the extern's declared logical parameter with the value the
/// entry field's own call site passed for it (positional against `params`);
/// the substituted value is rendered with an empty `(params, site_args)`, so
/// a stray `Param` inside it (which the grammar never produces) fails loudly
/// instead of silently matching the outer template's parameters.
fn render_arg(
    r: &mut Resolver,
    arg: &CallArg,
    params: &[ExternParam],
    site_args: &[CallArg],
) -> String {
    match arg {
        CallArg::Param(name) => {
            let idx = params
                .iter()
                .position(|p| &p.name == name)
                .unwrap_or_else(|| {
                    panic!("extern call template references undeclared parameter {name:?}")
                });
            let site = site_args.get(idx).unwrap_or_else(|| {
                panic!("extern call site is missing an argument for parameter {name:?}")
            });
            render_arg(r, site, &[], &[])
        }
        CallArg::Ref(path) => r.path_read(path),
        CallArg::Lit(v) => json_literal(v),
        CallArg::List(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| render_arg(r, i, params, site_args))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Ctor(CallCtor { fields, .. }) => format!(
            "{{ {} }}",
            fields
                .iter()
                .map(|(k, v)| format!("{k}: {}", render_arg(r, v, params, site_args)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        CallArg::Call(_) => {
            unimplemented!(
                "a nested extern call used as another call's argument is not supported yet"
            )
        }
    }
}

/// A path rooted at the single bound `yields` name, read off the awaited
/// call's raw result: the head (the `yields:` position name itself) becomes
/// `raw`, the rest stays verbatim (a foreign struct's own field names, never
/// cased).
fn foreign_path_expr(yields_name: &str, path: &[String]) -> String {
    let (head, rest) = path
        .split_first()
        .unwrap_or_else(|| panic!("a returns: value has no path segments"));
    assert_eq!(
        head, yields_name,
        "a returns: value references a yields name other than the one bound here, which is not supported yet"
    );
    if rest.is_empty() {
        "raw".to_string()
    } else {
        format!("raw.{}", rest.join("."))
    }
}

fn arm_value_expr(yields_name: &str, value: &ArmValue) -> String {
    match value {
        ArmValue::Field(path) => foreign_path_expr(yields_name, path),
        ArmValue::Lit(v) => json_literal(v),
        // A declared-source chain names an entry field's own resolution, not
        // a foreign value the extern call just produced; a `returns:` match
        // arm inside an extern binding has nothing to run that chain over.
        ArmValue::Sources(_) => {
            unreachable!("a returns: match arm cannot bind a declared-source chain")
        }
    }
}

/// A `match` inside `returns:`, lowered to an immediately invoked switch:
/// TypeScript has no match expression, so the arms return from a wrapper
/// function instead of assigning through a shared destination the way a
/// statement-level `switch` (`plan::Stmt::Switch`) does elsewhere in this
/// target. The subject and every arm read off the same awaited raw result
/// `returns:`'s bare-field case does.
fn select_expr(yields_name: &str, select: &Select) -> String {
    let subject = foreign_path_expr(yields_name, &select.subject);
    let mut arms = String::new();
    for arm in &select.arms {
        let value = arm_value_expr(yields_name, &arm.value);
        match &arm.pattern {
            Some(pattern) => arms.push_str(&format!(
                "    case {}: return {value};\n",
                json_literal(pattern)
            )),
            None => arms.push_str(&format!("    default: return {value};\n")),
        }
    }
    format!("(() => {{\n  switch ({subject}) {{\n{arms}  }}\n}})()")
}

/// A `returns:` field's value, projected off the single bound `yields` name.
fn returns_value_expr(yields_name: &str, value: &ReturnsValue) -> String {
    match value {
        ReturnsValue::Field(path) => foreign_path_expr(yields_name, path),
        ReturnsValue::Select(select) => select_expr(yields_name, select),
    }
}

/// [`plan::Emitter::call_assign`] for TypeScript: the try/catch around one
/// awaited extern call, its argument rendering, its `yields`/`returns`
/// projection (or bare pass-through when there is none), and its error
/// boundary. `dest` is already a ready-to-use assignment target (a top-level
/// field or a config member path), supplied by the shared plan.
pub(super) fn call_assign(
    r: &mut Resolver,
    field: &EntryField,
    call: &EntryCall,
    dest: &str,
) -> String {
    // `entries::validate_entries` (`call_resolves`) has already confirmed all
    // of this for the typescript target before generation reaches here, so
    // every lookup below is an internal-invariant check, not a validation.
    let lib = r
        .module
        .ext_libs
        .iter()
        .find(|l| l.name == call.ns)
        .unwrap_or_else(|| {
            panic!(
                "entry field {:?} calls undeclared ext lib {:?} (validate_entries should have rejected this)",
                field.name, call.ns
            )
        });
    let decl = lib
        .externs
        .iter()
        .find(|e| e.name == call.func)
        .unwrap_or_else(|| {
            panic!(
                "entry field {:?} calls undeclared extern {:?} in ext lib {:?} (validate_entries should have rejected this)",
                field.name, call.func, lib.name
            )
        });
    let lang = decl
        .langs
        .iter()
        .find(|l| l.lang == "ts" || l.lang == "typescript")
        .unwrap_or_else(|| {
            panic!(
                "extern {}.{} has no typescript binding (validate_entries should have rejected this)",
                call.ns, call.func
            )
        });
    let lib_path = lib
        .langs
        .iter()
        .find(|p| p.lang == "ts" || p.lang == "typescript")
        .unwrap_or_else(|| {
            panic!(
                "ext lib {:?} declares no typescript module path (validate_entries should have rejected this)",
                lib.name
            )
        });

    r.helpers.ext_refs.push(Symbol::imported(
        lang.symbol.clone(),
        lib_path.path.clone(),
        lang.symbol.clone(),
    ));
    r.helpers
        .ext_refs
        .push(module_symbol(&error_names().contract, r.module));

    let args = {
        let mut parts = Vec::with_capacity(lang.call_args.len());
        for a in &lang.call_args {
            parts.push(render_arg(r, a, &decl.params, &call.args));
        }
        parts.join(", ")
    };
    let call_name = format!("{}.{}", call.ns, call.func);

    // A `ts` binding never reads its own `error` yields position (the
    // catch below is the only error channel a thrown Promise rejection
    // gives this target); `validate_entries` guarantees at most one
    // non-error position, which is the only one worth projecting.
    let assign = match lang.yields.iter().find(|y| !y.is_error) {
        None => format!("{dest} = raw;"),
        Some(y) => {
            let returns = lang.returns.as_ref().unwrap_or_else(|| {
                panic!(
                    "extern {call_name} declares a yields position but no returns to project it into (validate_entries should have rejected this)"
                )
            });
            let projected = returns
                .fields
                .iter()
                .map(|f| {
                    format!(
                        "{}: {}",
                        field_camel(&f.name, r.config),
                        returns_value_expr(&y.name, &f.value)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            format!("{dest} = {{ {projected} }};")
        }
    };

    let mut cases = String::new();
    for eb in &lang.errors {
        let class_name = sentinel_error_class(&eb.r#type);
        r.helpers.ext_error_types.insert(eb.r#type.clone());
        r.helpers
            .ext_refs
            .push(module_symbol(&class_name, r.module));
        cases.push_str(&format!(
            "      case {sentinel:?}: throw new {class_name}(e);\n",
            sentinel = eb.sentinel,
        ));
    }
    let switch = if cases.is_empty() {
        String::new()
    } else {
        format!("  switch (e instanceof Error ? e.message : String(e)) {{\n{cases}  }}\n",)
    };

    format!(
        "try {{\n  const raw = await {symbol}({args});\n  {assign}\n}} catch (e) {{\n{switch}  throw new {contract}({call_name:?}, e);\n}}",
        symbol = lang.symbol,
        contract = error_names().contract,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::targets::typescript::entry::emit;
    use crate::codegen::targets::typescript::types::ts_casing;
    use crate::codegen::targets::typescript::TsRules;
    use crate::codegen::test_support::{member, rendered, structure};
    use crate::ir::{
        EntryField, ExtLib, ExternDecl, ExternLang as IrExternLang, ForeignField, ForeignStruct,
        LangPath, OpaqueType, Prim, ReturnsField, ReturnsLit, Shape, ShapeKind, Source, Tref,
    };
    use std::collections::BTreeMap;

    fn ef(name: &str, target: Tref, sources: Vec<Source>, call: Option<EntryCall>) -> EntryField {
        EntryField {
            name: name.into(),
            target,
            sources,
            format: None,
            transforms: vec![],
            select: None,
            call,
            binds: vec![],
            constraints: vec![],
            traits: vec![],
        }
    }

    fn app_config_shape() -> Shape {
        structure(
            "m#app_config",
            vec![
                member("endpoint", Tref::Prim(Prim::String), true),
                member("token", Tref::Prim(Prim::String), true),
            ],
        )
    }

    /// A worked `companyconfig`/`companybus` `ext` library pair: `load` (a
    /// `Ctor` argument, `yields`+`returns` projecting foreign field names
    /// onto `app_config`, a declared sentinel) and `connect` (a bare handle
    /// construction, no `yields`).
    fn appendix_ext_libs() -> Vec<ExtLib> {
        let mut load_ctor_fields = BTreeMap::new();
        load_ctor_fields.insert("region".to_string(), CallArg::Param("region".into()));
        load_ctor_fields.insert("service".to_string(), CallArg::Param("service".into()));
        let companyconfig = ExtLib {
            name: "companyconfig".into(),
            langs: vec![LangPath {
                lang: "ts".into(),
                path: "@company/config".into(),
            }],
            structs: vec![
                ForeignStruct {
                    name: "ts_opts".into(),
                    fields: vec![
                        ForeignField {
                            name: "region".into(),
                            r#type: Tref::Prim(Prim::String),
                        },
                        ForeignField {
                            name: "service".into(),
                            r#type: Tref::Prim(Prim::String),
                        },
                    ],
                },
                ForeignStruct {
                    name: "ts_config".into(),
                    fields: vec![
                        ForeignField {
                            name: "host".into(),
                            r#type: Tref::Prim(Prim::String),
                        },
                        ForeignField {
                            name: "token".into(),
                            r#type: Tref::Prim(Prim::String),
                        },
                    ],
                },
            ],
            types: vec![],
            externs: vec![ExternDecl {
                name: "load".into(),
                params: vec![
                    ExternParam {
                        name: "service".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                    ExternParam {
                        name: "region".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                ],
                r#return: Tref::Ref {
                    id: "m#app_config".into(),
                    args: vec![],
                },
                langs: vec![IrExternLang {
                    lang: "ts".into(),
                    symbol: "load".into(),
                    call_args: vec![CallArg::Ctor(CallCtor {
                        name: "ts_opts".into(),
                        fields: load_ctor_fields,
                    })],
                    yields: vec![crate::ir::YieldsPos {
                        name: "cfg".into(),
                        r#type: Some(Tref::Ref {
                            id: "companyconfig#ts_config".into(),
                            args: vec![],
                        }),
                        is_error: false,
                    }],
                    returns: Some(ReturnsLit {
                        r#type: Tref::Ref {
                            id: "m#app_config".into(),
                            args: vec![],
                        },
                        fields: vec![
                            ReturnsField {
                                name: "endpoint".into(),
                                value: ReturnsValue::Field(vec!["cfg".into(), "host".into()]),
                            },
                            ReturnsField {
                                name: "token".into(),
                                value: ReturnsValue::Field(vec!["cfg".into(), "token".into()]),
                            },
                        ],
                    }),
                    errors: vec![crate::ir::ErrorBinding {
                        sentinel: "BUSY".into(),
                        r#type: "overloaded".into(),
                    }],
                    sync: false,
                }],
            }],
        };
        let companybus = ExtLib {
            name: "companybus".into(),
            langs: vec![LangPath {
                lang: "ts".into(),
                path: "@company/bus".into(),
            }],
            structs: vec![],
            types: vec![OpaqueType {
                name: "publisher".into(),
                methods: vec![],
            }],
            externs: vec![ExternDecl {
                name: "connect".into(),
                params: vec![
                    ExternParam {
                        name: "endpoint".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                    ExternParam {
                        name: "token".into(),
                        r#type: Tref::Prim(Prim::String),
                    },
                ],
                r#return: Tref::Ref {
                    id: "companybus#publisher".into(),
                    args: vec![],
                },
                langs: vec![IrExternLang {
                    lang: "ts".into(),
                    symbol: "connect".into(),
                    call_args: vec![
                        CallArg::Param("endpoint".into()),
                        CallArg::Param("token".into()),
                    ],
                    yields: vec![],
                    returns: None,
                    errors: vec![],
                    sync: false,
                }],
            }],
        };
        vec![companyconfig, companybus]
    }

    /// `service`/`region` (`@arg`), `config` (a plain call, `load`-shaped),
    /// `bus` (a `@with`-fallback call onto the opaque handle,
    /// `connect`-shaped, reading `config`'s own resolved members).
    fn appendix_fields() -> Vec<EntryField> {
        vec![
            ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None),
            ef("region", Tref::Prim(Prim::String), vec![Source::Arg], None),
            ef(
                "config",
                Tref::Ref {
                    id: "m#app_config".into(),
                    args: vec![],
                },
                vec![],
                Some(EntryCall {
                    ns: "companyconfig".into(),
                    func: "load".into(),
                    args: vec![
                        CallArg::Ref(vec!["service".into()]),
                        CallArg::Ref(vec!["region".into()]),
                    ],
                }),
            ),
            {
                let mut bus = ef(
                    "bus",
                    Tref::Ref {
                        id: "companybus#publisher".into(),
                        args: vec![],
                    },
                    vec![Source::With],
                    Some(EntryCall {
                        ns: "companybus".into(),
                        func: "connect".into(),
                        args: vec![
                            CallArg::Ref(vec!["config".into(), "endpoint".into()]),
                            CallArg::Ref(vec!["config".into(), "token".into()]),
                        ],
                    }),
                );
                bus.sources = vec![Source::With];
                bus
            },
        ]
    }

    fn appendix_module(fields: Vec<EntryField>) -> Module {
        Module {
            tests: vec![],
            name: "m".into(),
            shapes: vec![
                app_config_shape(),
                Shape {
                    id: "m#client".into(),
                    kind: ShapeKind::Entry {
                        fields,
                        operations: vec![],
                    },
                    traits: vec![],
                },
            ],
            operations: vec![],
            extensions: vec![],
            ext_libs: appendix_ext_libs(),
        }
    }

    fn rendered_text(module: &Module) -> String {
        let emission = emit(module, &ts_casing());
        let mut decls = emission.shared;
        decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
        rendered(&decls, &TsRules)
    }

    #[test]
    fn a_plain_call_field_awaits_the_foreign_symbol() {
        // The import statement itself is a later file-assembly concern
        // (`repoint_to_groups`/`fill_symbol_slots`), not exercised by this
        // leaf-level harness; the ref that drives it is asserted separately
        // by checking the class `Decl`'s own `refs`.
        let module = appendix_module(appendix_fields());
        let decls = rendered_decls(&module);
        let client_decl = decls
            .iter()
            .find(|d| matches!(d, Decl::Raw(raw) if raw.text.contains("export class Client")))
            .expect("client class decl");
        let refs = crate::codegen::tree::item_refs(client_decl);
        assert!(
            refs.iter().any(|s| s.name == "load"
                && s.import.as_ref().is_some_and(|i| i.module == "@company/config")),
            "the client class must import the foreign symbol `load` from its declared module: {refs:?}"
        );
        let out = rendered_text(&module);
        assert!(out.contains("const raw = await load("), "{out}");
        // The `Ctor` argument's foreign field names ride verbatim, in the
        // ts language block's own order; the values are the resolved
        // sibling fields (`s.<field>`), not bare identifiers.
        assert!(
            out.contains("{ region: s.region, service: s.service }"),
            "{out}"
        );
    }

    #[test]
    fn a_yields_projection_reads_the_foreign_verbatim_member_and_casts_to_the_logical_type() {
        let out = rendered_text(&appendix_module(appendix_fields()));
        assert!(
            out.contains("s.config = { endpoint: raw.host, token: raw.token };"),
            "{out}"
        );
    }

    #[test]
    fn a_bare_call_with_no_yields_assigns_the_raw_result_directly() {
        let out = rendered_text(&appendix_module(appendix_fields()));
        // The `@with` fallback wraps the call assignment inside its own
        // presence check; the leaf itself is still a bare pass-through.
        assert!(out.contains("s.bus = raw;"), "{out}");
        assert!(out.contains("await connect("), "{out}");
    }

    #[test]
    fn a_declared_sentinel_throws_the_generated_typed_error() {
        let out = rendered_text(&appendix_module(appendix_fields()));
        assert!(
            out.contains("case \"BUSY\": throw new OverloadedError(e);"),
            "{out}"
        );
        assert!(
            out.contains("export class OverloadedError extends TonoError"),
            "{out}"
        );
    }

    #[test]
    fn an_unmapped_failure_falls_back_to_contract_error_naming_the_extern() {
        let out = rendered_text(&appendix_module(appendix_fields()));
        assert!(
            out.contains("throw new ContractError(\"companyconfig.load\", e);"),
            "{out}"
        );
        assert!(
            out.contains("throw new ContractError(\"companybus.connect\", e);"),
            "{out}"
        );
    }

    #[test]
    fn an_entry_with_a_call_field_gets_an_async_static_factory_constructor() {
        let out = rendered_text(&appendix_module(appendix_fields()));
        let client_at = out.find("export class Client").expect("client class");
        let client_text = &out[client_at..];
        assert!(client_text.contains("private constructor("), "{out}");
        assert!(client_text.contains("static async create("), "{out}");
        assert!(!client_text.contains("\n  constructor("), "{out}");
    }

    #[test]
    fn an_entry_with_no_call_field_keeps_the_plain_sync_constructor() {
        let fields = vec![ef(
            "service",
            Tref::Prim(Prim::String),
            vec![Source::Arg],
            None,
        )];
        let out = rendered_text(&appendix_module(fields));
        assert!(out.contains("\n  constructor("), "{out}");
        assert!(!out.contains("static async create("), "{out}");
        assert!(!out.contains("private constructor("), "{out}");
    }

    #[test]
    fn no_foreign_form_is_exported_by_the_barrel() {
        let module = appendix_module(appendix_fields());
        let decls = rendered_decls(&module);
        let exports = crate::codegen::targets::typescript::emit::exports_of(&decls);
        for name in ["ts_opts", "ts_config", "load", "connect"] {
            assert!(
                !exports.values.iter().any(|v| v == name)
                    && !exports.types.iter().any(|v| v == name),
                "the barrel must not export the foreign name {name:?}"
            );
        }
    }

    fn rendered_decls(module: &Module) -> Vec<Decl> {
        let emission = emit(module, &ts_casing());
        let mut decls = emission.shared;
        decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
        decls
    }

    /// A `match` inside `returns:` (the appendix's `.cfg.Env` example), the
    /// same construct a config member's own `= match` selection uses,
    /// lowered to an immediately invoked switch since TypeScript has no
    /// match expression.
    #[test]
    fn a_match_inside_returns_lowers_to_an_immediately_invoked_switch() {
        use crate::ir::{ArmValue, Select, SelectArm};

        let mut fields = std::collections::BTreeMap::new();
        fields.insert("service".to_string(), CallArg::Param("service".into()));
        let lib = ExtLib {
            name: "companyconfig".into(),
            langs: vec![crate::ir::LangPath {
                lang: "ts".into(),
                path: "@company/config".into(),
            }],
            structs: vec![],
            types: vec![],
            externs: vec![ExternDecl {
                name: "load".into(),
                params: vec![ExternParam {
                    name: "service".into(),
                    r#type: Tref::Prim(Prim::String),
                }],
                r#return: Tref::Ref {
                    id: "m#app_config".into(),
                    args: vec![],
                },
                langs: vec![crate::ir::ExternLang {
                    lang: "ts".into(),
                    symbol: "load".into(),
                    call_args: vec![CallArg::Param("service".into())],
                    yields: vec![crate::ir::YieldsPos {
                        name: "cfg".into(),
                        r#type: None,
                        is_error: false,
                    }],
                    returns: Some(ReturnsLit {
                        r#type: Tref::Ref {
                            id: "m#app_config".into(),
                            args: vec![],
                        },
                        fields: vec![ReturnsField {
                            name: "endpoint".into(),
                            value: ReturnsValue::Select(Select {
                                subject: vec!["cfg".into(), "Env".into()],
                                arms: vec![
                                    SelectArm {
                                        pattern: Some(serde_json::json!("prod")),
                                        value: ArmValue::Field(vec!["cfg".into(), "Host".into()]),
                                    },
                                    SelectArm {
                                        pattern: None,
                                        value: ArmValue::Field(vec![
                                            "cfg".into(),
                                            "DevHost".into(),
                                        ]),
                                    },
                                ],
                            }),
                        }],
                    }),
                    errors: vec![],
                    sync: false,
                }],
            }],
        };
        let config = ef(
            "config",
            Tref::Ref {
                id: "m#app_config".into(),
                args: vec![],
            },
            vec![],
            Some(EntryCall {
                ns: "companyconfig".into(),
                func: "load".into(),
                args: vec![CallArg::Ref(vec!["service".into()])],
            }),
        );
        let service = ef("service", Tref::Prim(Prim::String), vec![Source::Arg], None);
        let mut module = appendix_module(vec![service, config]);
        module.ext_libs = vec![lib];
        let out = rendered_text(&module);
        assert!(out.contains("s.config = { endpoint: (() => {"), "{out}");
        assert!(out.contains("switch (raw.Env) {"), "{out}");
        assert!(out.contains("case \"prod\": return raw.Host;"), "{out}");
        assert!(out.contains("default: return raw.DevHost;"), "{out}");
        assert!(out.contains("})() };"), "{out}");
    }
}

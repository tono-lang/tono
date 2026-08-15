//! The `call_assign`/`impl_call_body` happy-path (a full appendix-like
//! module through `emit`) and defensive-fallback tests, split out of
//! `ext_tests` to keep it under the file-size ceiling; the fixtures
//! (`field`, `structure`, `bare_module`, `handle_lib`, `entry_text`, ...)
//! come from the parent module via `super::*`.

use super::*;

// --- call_assign / impl_call_body: happy paths and defensive fallbacks -

#[test]
fn the_appendix_module_wires_the_call_handle_and_impl_call() {
    // The exact RFC-0023 appendix fixture the go_ext_roundtrip integration
    // test also `go build`s against real libraries (shared via
    // `ext_fixtures::rfc0023_appendix_module`, so the two never drift):
    // this test checks the generated Go statements directly, exercised
    // through the full `emit` pipeline so the integration with
    // `mod.rs`/`surface.rs`/`constructor.rs`/`resolve.rs` is covered too,
    // not just `ext.rs` in isolation.
    let module = crate::codegen::targets::go::entry::ext_fixtures::rfc0023_appendix_module();
    let text = entry_text(&module);

    // Field-construction call: import, call, yields naming, returns
    // projection with a hoisted match.
    assert!(text.contains(":= companyconfig.Load(s.Service, s.Region)"));
    assert!(text.contains("configErr != nil"));
    assert!(text.contains("switch configCfg.Env {"));
    assert!(text.contains("var configEndpoint string"));
    assert!(text.contains("s.Config = AppConfig{"));
    assert!(text.contains("Token: configCfg.Credentials.Secret"));

    // The handle field is unexported, its With* option assigns without a
    // dereference (the carrier already holds the pointer type), and the
    // construction fallback wraps the call.
    assert!(text.contains("\tbus *companybus.Publisher\n"));
    assert!(!text.contains("\tBus "));
    assert!(text.contains("func WithBus(v *companybus.Publisher) ClientOption {"));
    assert!(text.contains("w.bus = v"));
    assert!(text.contains("if w.bus != nil {"));
    assert!(text.contains("s.bus = w.bus"));
    assert!(text.contains(":= companybus.Connect(s.Config.Endpoint, s.Config.Token)"));

    // The op's own impl call: reads the receiver, calls the method, maps
    // the declared sentinel, and projects the return.
    assert!(text.contains("c.settings.bus"));
    assert!(text.contains(".Send(\"notes\", input.Body)"));
    assert!(text.contains("errors.Is(publishErr, companybus.ErrBusy)"));
    assert!(text.contains("&Overloaded{Message: publishErr.Error()}"));
    assert!(text.contains("ContractName: \"companybus.publisher.send\""));
    assert!(text.contains("return Ack{"));
}

// --- defensive fallbacks, exercised directly ---------------------------

/// A declared extern with no module path for `lang` (the "no Go binding"
/// case leaves the lib's own `langs` empty and binds `fn_name` for `ts`
/// only; the "no Go module path" case binds `fn_name` for `go` but leaves
/// the lib's own `langs` empty), so `go_lang`/`import_lib` each miss on
/// exactly the one thing they check.
fn lib_missing(lib_langs: Vec<LangPath>, fn_name: &str, fn_lang: &str) -> ExtLib {
    ExtLib {
        name: "nope".into(),
        langs: lib_langs,
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: fn_name.into(),
            params: vec![],
            r#return: string_t(),
            langs: vec![ExternLang {
                lang: fn_lang.into(),
                symbol: fn_name.into(),
                call_args: vec![],
                yields: vec![],
                returns: None,
                errors: vec![],
            }],
        }],
    }
}

/// A one-entry module whose single field carries `call`, plus the field
/// itself: the shared shape every `call_assign` case below builds a
/// `Resolver` over.
fn module_with_call_field(ext_libs: Vec<ExtLib>, call: EntryCall) -> (Module, EntryField) {
    let mut config_field = field("config", string_t(), vec![]);
    config_field.call = Some(call);
    let mut module = bare_module();
    module.ext_libs = ext_libs;
    module.shapes.push(Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields: vec![config_field.clone()],
            operations: vec![],
        },
        traits: vec![],
    });
    (module, config_field)
}

/// Build the entry, the `Resolver`, and run `call_assign` for `config_field`
/// against `module` in one call, so each case below reads as the one thing
/// that differs (the module's own `ext_libs`) rather than repeating the
/// `Resolver` plumbing.
fn call_assign_for(module: &Module, config_field: &EntryField) -> String {
    let entries = module_entries(module);
    let entry = &entries[0];
    let config = go_casing();
    let mut r = Resolver {
        entry,
        module,
        config: &config,
        helpers: &mut Helpers::default(),
        refs: &mut Vec::new(),
        body: &mut String::new(),
        resolve_fns: &mut Vec::new(),
        multi: false,
    };
    let call = config_field.call.clone().unwrap();
    ext::call_assign(&mut r, config_field, &call, "s.Config")
}

#[test]
fn call_assign_degrades_on_every_unresolved_lookup() {
    let call = EntryCall {
        ns: "nope".into(),
        func: "load".into(),
        args: vec![],
    };

    let (module, config_field) = module_with_call_field(vec![], call.clone());
    assert!(call_assign_for(&module, &config_field).contains("unresolved ext lib"));

    let (module, config_field) =
        module_with_call_field(vec![handle_lib("nope", "publisher")], call.clone());
    assert!(call_assign_for(&module, &config_field).contains("unresolved extern"));

    let ts_only = lib_missing(
        vec![LangPath {
            lang: "ts".into(),
            path: "@company/nope".into(),
        }],
        "load",
        "ts",
    );
    let (module, config_field) = module_with_call_field(vec![ts_only], call.clone());
    assert!(call_assign_for(&module, &config_field).contains("declares no Go binding"));

    let no_go_path = lib_missing(vec![], "load", "go");
    let (module, config_field) = module_with_call_field(vec![no_go_path], call);
    assert!(call_assign_for(&module, &config_field).contains("declares no Go module path"));
}

#[test]
fn call_assign_covers_an_explicit_error_position_and_a_bare_call_result() {
    let lib = ExtLib {
        name: "lib".into(),
        langs: vec![LangPath {
            lang: "go".into(),
            path: "company/lib".into(),
        }],
        structs: vec![],
        types: vec![],
        externs: vec![ExternDecl {
            name: "fetch".into(),
            params: vec![],
            r#return: string_t(),
            langs: vec![ExternLang {
                lang: "go".into(),
                symbol: "Fetch".into(),
                call_args: vec![],
                // The error position leads instead of trailing (ADR-0031
                // out-of-convention case), and there is no `returns:`,
                // so the bare call result assigns directly.
                yields: vec![
                    YieldsPos {
                        name: "err".into(),
                        r#type: None,
                        is_error: true,
                    },
                    YieldsPos {
                        name: "body".into(),
                        r#type: Some(string_t()),
                        is_error: false,
                    },
                ],
                returns: None,
                errors: vec![],
            }],
        }],
    };
    let call = EntryCall {
        ns: "lib".into(),
        func: "fetch".into(),
        args: vec![],
    };
    let (module, config_field) = module_with_call_field(vec![lib], call);
    let out = call_assign_for(&module, &config_field);
    assert!(out.contains("configErr, configBody := lib.Fetch()"));
    assert!(out.contains("s.Config = configBody"));
}

/// A single-field entry named `bus`, targeting the declared handle, with
/// every other field the fixture needs alongside it.
fn module_with_bus_field(ext_libs: Vec<ExtLib>, other_fields: Vec<EntryField>) -> Module {
    let mut bus_field = field(
        "bus",
        Tref::Ref {
            id: "bus#publisher".into(),
            args: vec![],
        },
        vec![],
    );
    bus_field.name = "bus".into();
    let mut fields = other_fields;
    fields.push(bus_field);
    let mut module = bare_module();
    module.ext_libs = ext_libs;
    module.shapes.push(Shape {
        id: "m#client".into(),
        kind: ShapeKind::Entry {
            fields,
            operations: vec![],
        },
        traits: vec![],
    });
    module
}

/// Build the entry and run `impl_call_body` for `call` against `module` in
/// one call, so each case below reads as the one thing that differs.
fn impl_call_for(module: &Module, input_name: Option<&str>, call: &OpImplCall) -> String {
    let entries = module_entries(module);
    let entry = &entries[0];
    let config = go_casing();
    ext::impl_call_body(
        entry,
        module,
        &config,
        "publish",
        input_name,
        call,
        "nil, ",
        &|e| e,
        &mut Vec::new(),
    )
}

#[test]
fn impl_call_body_degrades_on_every_unresolved_lookup() {
    // No handle field at all: enough to exercise "no receiver" and
    // "receiver field not found" (`EntryModel::fields` is non-empty from
    // `region`, but names nothing a receiver path could match).
    let plain = module_with_bus_field(vec![], vec![field("region", string_t(), vec![])]);
    let no_recv = OpImplCall {
        recv: vec![],
        method: "send".into(),
        args: vec![],
    };
    assert!(impl_call_for(&plain, None, &no_recv).contains("no receiver"));
    let missing = OpImplCall {
        recv: vec!["missing".into()],
        method: "send".into(),
        args: vec![],
    };
    assert!(impl_call_for(&plain, None, &missing).contains("unresolved receiver field"));
    let not_handle = OpImplCall {
        recv: vec!["region".into()],
        method: "send".into(),
        args: vec![],
    };
    assert!(impl_call_for(&plain, None, &not_handle).contains("is not a foreign handle field"));

    // A declared handle field whose method is not declared, and whose
    // declared method has no Go binding.
    let with_bus = module_with_bus_field(vec![handle_lib("bus", "publisher")], vec![]);
    let missing_method = OpImplCall {
        recv: vec!["bus".into()],
        method: "missing".into(),
        args: vec![],
    };
    assert!(impl_call_for(&with_bus, None, &missing_method).contains("unresolved method"));

    let mut ts_only_bus = with_bus.clone();
    ts_only_bus.ext_libs[0].types[0].methods[0].langs[0].lang = "ts".into();
    let send = OpImplCall {
        recv: vec!["bus".into()],
        method: "send".into(),
        args: vec![],
    };
    assert!(impl_call_for(&ts_only_bus, None, &send).contains("declares no Go binding"));
}

#[test]
fn impl_call_body_reads_a_ref_argument_off_the_input_param_and_off_an_entry_field() {
    let mut lib = handle_lib("bus", "publisher");
    lib.types[0].methods[0].params = vec![
        ext_param("body", string_t()),
        ext_param("region", string_t()),
    ];
    lib.types[0].methods[0].langs[0].call_args = vec![
        CallArg::Param("body".into()),
        CallArg::Param("region".into()),
    ];
    let module = module_with_bus_field(vec![lib], vec![field("region", string_t(), vec![])]);

    // Bare param reference (`.payload`, whole value) and a sibling entry
    // field reference (`.region`) in the same call.
    let call = OpImplCall {
        recv: vec!["bus".into()],
        method: "send".into(),
        args: vec![
            CallArg::Ref(vec!["payload".into()]),
            CallArg::Ref(vec!["region".into()]),
        ],
    };
    let out = impl_call_for(&module, Some("payload"), &call);
    assert!(out.contains(".Send(input, c.settings.Region)"));
}

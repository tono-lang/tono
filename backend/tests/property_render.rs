//! Property tests for the printers: an arbitrary well-formed component tree
//! renders, in every target, to text the language's official formatter parses.
//!
//! The formatter is a real parser of the target language, so a rejection means
//! the render rules spelled invalid syntax — the exact class of bug the typed
//! tree cannot rule out on its own, exercised here under randomized nesting
//! (`list<map<K, nullable<V>>>`, generics in generics, unions in collections)
//! rather than the flat golden fixtures. `Decl::Raw` and `Decl::Function`
//! carry opaque target text, so arbitrary instances would blame the printer
//! for garbage it never produced; they are the only nodes left out. Symbol
//! references may be unresolvable — the oracle checks syntax, not name
//! resolution. A target whose formatter is not installed is skipped, the same
//! stance the native-check test takes toward missing toolchains.

use std::path::Path;

use proptest::collection::vec;
use proptest::option;
use proptest::prelude::*;
use proptest::sample::select;
use proptest::test_runner::{Config, TestRunner};
use tono_backend::codegen::render::render_file;
use tono_backend::codegen::symbol::Symbol;
use tono_backend::codegen::target::RenderRules;
use tono_backend::codegen::targets::go::emit::package_clause;
use tono_backend::codegen::targets::go::GoRules;
use tono_backend::codegen::targets::rust::render::RustRules;
use tono_backend::codegen::targets::typescript::render::TsRules;
use tono_backend::codegen::tree::{
    ClientDecl, Decl, EnumDecl, EnumRepr, Field, File, Interface, Method, TypeExpr, UnionDecl,
    Variant,
};
use tono_backend::codegen::{Formatter, TargetKind, Warning};

// Small fixed pools keep generated trees well-formed while exercising every
// node, mirroring property_ir.rs. Identifiers avoid every target's keywords
// (`type`, `fn`, `map`, ...) so a failure always means bad spelling, never a
// reserved name the real emitters could not produce either.
fn type_symbol() -> impl Strategy<Value = Symbol> {
    prop_oneof![
        select(vec!["Widget", "Item", "PageInfo", "string", "bool"]).prop_map(Symbol::builtin),
        select(vec!["Charge", "Refund"]).prop_map(|n| Symbol::imported(n, "alpha", n)),
    ]
}
fn generic_symbol() -> impl Strategy<Value = Symbol> {
    select(vec!["Page", "Batch"]).prop_map(|n| Symbol::imported(n, "alpha", n))
}
fn field_symbol() -> impl Strategy<Value = Symbol> {
    select(vec!["amount", "note", "total", "items", "label"]).prop_map(Symbol::builtin)
}
fn method_symbol() -> impl Strategy<Value = Symbol> {
    select(vec!["fetch", "create", "list_all"]).prop_map(Symbol::builtin)
}
fn wire() -> impl Strategy<Value = String> {
    select(vec!["amount_due", "kind", "x-key"]).prop_map(String::from)
}
fn doc() -> impl Strategy<Value = String> {
    select(vec!["One line of docs.", "Why this exists."]).prop_map(String::from)
}
fn reason() -> impl Strategy<Value = String> {
    select(vec!["", "use v2 instead"]).prop_map(String::from)
}

fn type_expr() -> impl Strategy<Value = TypeExpr> {
    let leaf = type_symbol().prop_map(TypeExpr::Ref);
    // Depth 3, up to 16 nodes: enough to compose every constructor through
    // every other without exploding the case size.
    leaf.prop_recursive(3, 16, 2, |inner| {
        prop_oneof![
            inner.clone().prop_map(|t| TypeExpr::List(Box::new(t))),
            (inner.clone(), inner.clone())
                .prop_map(|(k, v)| TypeExpr::Map(Box::new(k), Box::new(v))),
            inner.clone().prop_map(|t| TypeExpr::Nullable(Box::new(t))),
            // A generic reference always applies at least one argument; a bare
            // `Page[]` is not a shape any emitter produces.
            (generic_symbol(), vec(inner.clone(), 1..3))
                .prop_map(|(s, args)| TypeExpr::Generic(s, args)),
            (inner.clone(), inner).prop_map(|(k, v)| TypeExpr::Entries(Box::new(k), Box::new(v))),
        ]
    })
}

fn field() -> impl Strategy<Value = Field> {
    (
        field_symbol(),
        type_expr(),
        any::<bool>(),
        option::of(wire()),
        option::of(reason()),
        option::of(doc()),
    )
        .prop_map(|(name, ty, nullable, wire, deprecated, doc)| Field {
            tag: None,
            name,
            ty,
            nullable,
            wire,
            deprecated,
            doc,
        })
}

// A method/function parameter never carries deprecation or docs (the tree's
// own invariant), so the generator respects it rather than probing outside
// the emitters' reachable domain.
fn param() -> impl Strategy<Value = Field> {
    (field_symbol(), type_expr(), any::<bool>()).prop_map(|(name, ty, nullable)| Field {
        tag: None,
        name,
        ty,
        nullable,
        wire: None,
        deprecated: None,
        doc: None,
    })
}

fn interface() -> impl Strategy<Value = Interface> {
    (
        type_symbol(),
        // Fixed distinct sets: a duplicated parameter is a semantic error no
        // emitter produces, and syntax is the axis under test.
        select(vec![
            Vec::new(),
            vec!["T".to_string()],
            vec!["T".to_string(), "U".to_string()],
        ]),
        vec(field(), 0..4),
        option::of(reason()),
        option::of(doc()),
    )
        .prop_map(|(name, params, fields, deprecated, doc)| Interface {
            name,
            params,
            fields,
            deprecated,
            doc,
        })
}

fn enum_decl() -> impl Strategy<Value = EnumDecl> {
    (
        type_symbol(),
        // Per-member tuples keep `members`, `member_docs`, and the int backing
        // parallel by construction. At least one member: an empty enum is not
        // expressible in a spec.
        vec(
            (
                select(vec!["Active", "Closed", "Pending", "Failed"]).prop_map(Symbol::builtin),
                option::of(doc()),
                0i64..100,
            ),
            1..4,
        ),
        any::<bool>(),
        option::of(reason()),
        option::of(doc()),
    )
        .prop_map(|(name, members, int_backed, deprecated, doc)| {
            let mut symbols = Vec::new();
            let mut docs = Vec::new();
            let mut ints = Vec::new();
            for (symbol, doc, int) in members {
                symbols.push(symbol);
                docs.push(doc);
                ints.push(int);
            }
            EnumDecl {
                name,
                members: symbols,
                member_docs: docs,
                backing: if int_backed {
                    EnumRepr::Int(ints)
                } else {
                    EnumRepr::String
                },
                deprecated,
                doc,
            }
        })
}

fn variant() -> impl Strategy<Value = Variant> {
    (
        select(vec!["Card", "Pix", "Boleto"]).prop_map(Symbol::builtin),
        // Inline fields and a payload shape are alternatives, never both.
        prop_oneof![
            vec(field(), 0..3).prop_map(|fields| (fields, None)),
            type_expr().prop_map(|payload| (Vec::new(), Some(payload))),
        ],
        option::of(wire()),
        option::of(doc()),
    )
        .prop_map(|(name, (fields, payload), wire, doc)| Variant {
            name,
            fields,
            payload,
            wire,
            doc,
        })
}

fn union_decl() -> impl Strategy<Value = UnionDecl> {
    (
        type_symbol(),
        select(vec!["type", "kind"]).prop_map(String::from),
        vec(variant(), 1..3),
        option::of(reason()),
        option::of(doc()),
    )
        .prop_map(
            |(name, discriminator, variants, deprecated, doc)| UnionDecl {
                name,
                discriminator,
                variants,
                deprecated,
                doc,
            },
        )
}

fn method() -> impl Strategy<Value = Method> {
    (
        method_symbol(),
        vec(param(), 0..3),
        option::of(type_expr()),
        option::of(type_expr()),
        any::<bool>(),
        option::of(doc()),
    )
        .prop_map(|(name, params, ret, err, is_async, doc)| Method {
            name,
            params,
            ret,
            err,
            is_async,
            doc,
        })
}

fn decl() -> impl Strategy<Value = Decl> {
    prop_oneof![
        interface().prop_map(Decl::Interface),
        enum_decl().prop_map(Decl::Enum),
        union_decl().prop_map(Decl::Union),
        method().prop_map(Decl::Method),
        (type_symbol(), vec(method(), 1..3))
            .prop_map(|(name, methods)| Decl::Client(ClientDecl { name, methods })),
        // The alias definition is target text; `string` is a type token every
        // target parses, so the node itself is still exercised.
        type_symbol().prop_map(|name| {
            Decl::Alias(tono_backend::codegen::tree::Alias {
                name,
                value: "string".to_string(),
            })
        }),
    ]
}

fn file() -> impl Strategy<Value = File> {
    vec(decl(), 1..5).prop_map(|decls| File {
        module: "billing".to_string(),
        decls,
    })
}

/// The rough text the pipeline would hand the formatter for `file` in `kind`:
/// rendered through the target's real rules with a passthrough formatter, plus
/// the package clause generation prepends for Go (a Go file does not parse
/// without one).
fn rough_text(kind: TargetKind, file: &File) -> String {
    let passthrough = Formatter::new("cat", vec![]);
    let rules: Box<dyn RenderRules> = match kind {
        TargetKind::Go => Box::new(GoRules::default()),
        TargetKind::Rust => Box::new(RustRules::default()),
        TargetKind::TypeScript => Box::new(TsRules),
    };
    let rough = render_file(file, rules.as_ref(), &passthrough).text;
    match kind {
        TargetKind::Go => format!("{}{rough}", package_clause(&file.module)),
        _ => rough,
    }
}

/// The oracle formatter for `kind`. Prettier resolves through the checked-in
/// node workspace first (the same one the round-trip test uses) so the test
/// works wherever `npm ci` ran, falling back to `PATH`.
fn oracle(kind: TargetKind) -> Formatter {
    if kind == TargetKind::TypeScript {
        let local = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("codegen-tests/typescript/node_modules/.bin/prettier");
        if local.exists() {
            return Formatter::new(
                local.to_string_lossy().into_owned(),
                vec!["--parser".into(), "typescript".into()],
            );
        }
    }
    Formatter::for_target(kind)
}

/// A minimal file each target's formatter must accept, proving the binary is
/// present and working before the property loop leans on it.
fn probe_text(kind: TargetKind) -> &'static str {
    match kind {
        TargetKind::Go => "package p\n",
        TargetKind::Rust => "pub struct P;\n",
        TargetKind::TypeScript => "export const p = 1;\n",
    }
}

fn printer_emits_valid_syntax(kind: TargetKind) {
    let formatter = oracle(kind);
    match formatter.run(probe_text(kind)).warning {
        None => {}
        Some(Warning::FormatterUnavailable { program }) => {
            eprintln!("skipping {kind:?}: {program} is not installed");
            return;
        }
        Some(Warning::FormatterRejected {
            program, stderr, ..
        }) => {
            panic!("{program} rejected its own probe file: {stderr}")
        }
    }
    let mut runner = TestRunner::new(Config::with_cases(200));
    let result = runner.run(&file(), |file| {
        let rough = rough_text(kind, &file);
        if let Some(Warning::FormatterRejected {
            program, stderr, ..
        }) = formatter.run(&rough).warning
        {
            return Err(proptest::test_runner::TestCaseError::fail(format!(
                "{program} rejected the rendered source:\n--- stderr ---\n{stderr}\n--- rough ---\n{rough}"
            )));
        }
        Ok(())
    });
    if let Err(failure) = result {
        panic!("{kind:?} printer emitted invalid syntax: {failure}");
    }
}

#[test]
fn go_printer_emits_syntax_gofmt_accepts() {
    printer_emits_valid_syntax(TargetKind::Go);
}

#[test]
fn rust_printer_emits_syntax_rustfmt_accepts() {
    printer_emits_valid_syntax(TargetKind::Rust);
}

#[test]
fn typescript_printer_emits_syntax_prettier_accepts() {
    printer_emits_valid_syntax(TargetKind::TypeScript);
}

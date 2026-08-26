//! Native `go test` files generated from the module's declared tests.
//!
//! Each test runs the real client method over a client it assembled itself:
//! the constructor's shared settings step, then a fake assigned where every
//! stubbed foreign construction would have stored its value, the canonical
//! transport assigned where an `.http` stub answers, then the constructor's
//! last step. An `.impl` stub swaps the per-operation seam variable
//! ([`super::impl_op::impl_seam_var`]). A test whose call has no stub runs
//! against the real dependency, so it lands in the live file (`//go:build
//! live`) and stays out of a default `go test` run; a construction-only test
//! is hermetic by nature.
//!
//! The test files sit in the generated package itself (`_test.go` beside the
//! client), which is what lets them reach the unexported steps and assign the
//! unexported handle fields while the shipped surface stays clean. Every test
//! body is self-contained: the files declare no helper functions, so the test
//! files of two entries in one module (one Go package) never redefine a
//! symbol.

use std::collections::BTreeMap;

use crate::codegen::casing::CasingConfig;
use crate::codegen::declared_tests::{self, PlannedTest};
use crate::codegen::entries::plan;
use crate::codegen::group::Group;
use crate::codegen::ops::{error_names, op_io};
use crate::codegen::symbol::Symbol;
use crate::codegen::tree::{Decl, ModuleFile};
use crate::ir::{
    EnvName, ExtLib, ExternStubTarget, HttpAnswer, Module, OpaqueType, Source, StubAnswer, StubDep,
    TestPattern, Tref,
};

use super::surface::{method_name, with_option_name};
use super::{camel, ext, go_type, import, pascal, push_type_symbols, support_symbol, EntryModel};

#[path = "vector_expects.rs"]
mod expects;
use expects::{outcome_asserts, request_asserts};

/// The Go module path of the bespoke-outcome runtime a raw impl speaks.
const EXT_RUNTIME_MODULE: &str = "github.com/tono-lang/tono/runtimes/ext-go";

const BINDING_LANGS: [&str; 1] = ["go"];

/// The generated test files of a module's entries: one hermetic and one live
/// file per entry that declares tests, and nothing at all for one that has
/// none.
pub(crate) fn test_files(module: &Module, config: &CasingConfig) -> Vec<ModuleFile> {
    let Some((entries, multi, _bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
        return Vec::new();
    };
    // The pipeline validates the declared tests before any emitter runs; a
    // caller that drives this emitter directly with an invalid model gets the
    // same named refusal from the pipeline's own validation.
    let Ok(planned) = declared_tests::entry_tests(module) else {
        return Vec::new();
    };
    let mut files = Vec::new();
    for entry in &entries {
        let Some(group) = planned.iter().find(|g| g.entry == entry.name) else {
            continue;
        };
        let n = super::names(entry, multi);
        let mut hermetic = Vec::new();
        let mut live = Vec::new();
        // The env-unset idiom explains itself once per generated file.
        let mut first_unset = true;
        for test in &group.tests {
            let ctx = TestCtx {
                entry,
                n: &n,
                module,
                config,
                multi,
                test,
            };
            if test.hermetic {
                hermetic.extend(hermetic_test_decl(&ctx, &mut first_unset));
            } else {
                live.push(live_test_decl(&ctx));
            }
        }
        if !hermetic.is_empty() {
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, false),
                hermetic,
            ));
        }
        if !live.is_empty() {
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, true),
                live,
            ));
        }
    }
    files
}

/// Everything one test's function needs from its surroundings.
struct TestCtx<'a> {
    entry: &'a EntryModel<'a>,
    n: &'a super::Names,
    module: &'a Module,
    config: &'a CasingConfig,
    multi: bool,
    test: &'a PlannedTest<'a>,
}

impl TestCtx<'_> {
    fn values(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.test.construction.values
    }
}

/// `TestSaveNoteTheGlueGuards...`: the called operation (when there is one)
/// plus the test name, each reduced to Go identifier characters. Live tests
/// carry a `Live` suffix: a `-tags live` run compiles both files, so the same
/// test name must not produce the same function twice. A multi-entry module
/// prefixes the entry name: sibling entries' test files share one Go package,
/// so equal test names must not collide across them either.
fn test_fn_name(ctx: &TestCtx<'_>) -> String {
    let words: String = ctx
        .test
        .name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect();
    let entry = if ctx.multi {
        pascal(ctx.entry.name)
    } else {
        String::new()
    };
    match ctx.test.op {
        Some(op) => format!(
            "Test{entry}{}{words}",
            pascal(declared_tests::bare_op_name(&op.id))
        ),
        None => format!("Test{entry}{words}"),
    }
}

/// A Go string literal carrying arbitrary text: a raw string when possible, a
/// concatenation working around the one character a raw string cannot hold.
fn go_string(text: &str) -> String {
    if !text.contains('`') {
        return format!("`{text}`");
    }
    let parts: Vec<String> = text.split('`').map(|p| format!("`{p}`")).collect();
    parts.join(" + \"`\" + ")
}

fn json_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// The environment pinning of a hermetic test: every `@env` name a pinned
/// construction value maps to is set, and every other literal env name the
/// entry could read is cleared, so the test resolves the same values on any
/// machine.
fn env_pinning(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>, first_unset: &mut bool) -> String {
    let mut lines = String::new();
    for field in &ctx.entry.fields {
        let covered = ctx.values().get(&field.name);
        let mut pinned = false;
        for source in &field.sources {
            let Source::Env(EnvName::Name(name)) = source else {
                continue;
            };
            match covered {
                Some(value) if !pinned => {
                    lines.push_str(&format!(
                        "\tt.Setenv({name:?}, {value})\n",
                        value = go_string(&env_value(value)),
                    ));
                    pinned = true;
                }
                _ => {
                    refs.push(import("os", "os"));
                    if *first_unset {
                        lines.push_str(
                            "\t// Setenv records the restore; Unsetenv makes the variable truly absent.\n",
                        );
                        *first_unset = false;
                    }
                    lines.push_str(&format!(
                        "\tt.Setenv({name:?}, \"\")\n\tos.Unsetenv({name:?})\n"
                    ));
                }
            }
        }
    }
    lines
}

/// The string an env var is set to: a JSON string verbatim, anything else in
/// its JSON spelling (env values are text by nature).
fn env_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => json_text(other),
    }
}

/// The constructor arguments and options a test passes, from the pinned
/// construction values: `@arg` fields positionally, `@with` fields as options.
/// An unpinned `@arg` gets the type's zero value: its declared chain has
/// nothing else to resolve from, and the zero value keeps a construction-
/// failure expectation expressible.
fn construction_args(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> (String, String) {
    let args: Vec<String> = ctx
        .entry
        .args()
        .iter()
        .map(|f| {
            push_type_symbols(&f.target, refs);
            match ctx.values().get(&f.name) {
                Some(v) => pinned_literal(ctx.module, ctx.config, &f.target, v),
                None => zero_literal(ctx.module, &f.target),
            }
        })
        .collect();
    let opts: Vec<String> = ctx
        .entry
        .with_fields()
        .iter()
        .filter_map(|f| {
            let v = ctx.values().get(&f.name)?;
            push_type_symbols(&f.target, refs);
            Some(format!(
                "{}({})",
                with_option_name(ctx.entry.name, f, ctx.multi),
                pinned_literal(ctx.module, ctx.config, &f.target, v)
            ))
        })
        .collect();
    (args.join(", "), opts.join(", "))
}

fn join_args(head: &str, parts: &[&str]) -> String {
    let tail: Vec<&str> = parts.iter().filter(|p| !p.is_empty()).copied().collect();
    if tail.is_empty() {
        head.to_string()
    } else if head.is_empty() {
        tail.join(", ")
    } else {
        format!("{head}, {}", tail.join(", "))
    }
}

/// The `.http` stub: the canned responses answered per call (the last one
/// repeats, which is how `@retry` resilience is exercised), with every request
/// recorded for the `requests` expectations. A single answer returns directly,
/// with no sequence machinery.
fn transport_block(answers: &[&HttpAnswer]) -> String {
    let request = super::shared_slot("HTTPRequest");
    let response = super::shared_slot("HTTPResponse");
    let literal = |a: &&HttpAnswer| {
        let headers: String = a
            .headers
            .iter()
            .map(|(k, v)| format!("{k:?}: {v:?}, "))
            .collect();
        format!(
            "{{Status: {status}, Headers: map[string]string{{{headers}}}, Body: {body}}}",
            status = a.status,
            body = go_string(&a.body),
        )
    };
    let (responses_decl, closure_body) = match answers {
        [only] => (
            String::new(),
            format!("\t\treturn {response}{}, nil\n", literal(only)),
        ),
        _ => {
            let list: Vec<String> = answers.iter().map(literal).collect();
            (
                format!(
                    "\tresponses := []{response}{{{list}}}\n",
                    list = list.join(", "),
                ),
                "\t\ti := len(seen) - 1\n\
                 \t\tif i >= len(responses) {\n\t\t\ti = len(responses) - 1\n\t\t}\n\
                 \t\treturn responses[i], nil\n"
                    .to_string(),
            )
        }
    };
    format!(
        "\tvar seen []{request}\n\
         {responses_decl}\
         \ttransport := func(ctx context.Context, req {request}) ({response}, error) {{\n\
         \t\tseen = append(seen, req)\n\
         {closure_body}\
         \t}}\n"
    )
}

/// One more tab on every line, for a body moving into a `switch` arm.
fn indent_go(block: &str) -> String {
    block.lines().map(|l| format!("\t{l}\n")).collect()
}

/// The `.impl` stub: swap the operation's seam variable for the canned
/// answers, restoring it when the test finishes (the seam is package state, so
/// these tests do not run in parallel). A sequence answers per call, the last
/// one repeating.
fn impl_stub_block(ctx: &TestCtx<'_>, answers: &[StubAnswer], refs: &mut Vec<Symbol>) -> String {
    let op = ctx.test.op.expect("an impl stub rides a call");
    let seam = super::impl_op::impl_seam_var(ctx.n, op);
    let raw = ctx
        .module
        .extensions
        .iter()
        .find(|e| {
            e.kind == crate::ir::ExtKind::Impl
                && declared_tests::bare_op_name(&op.id) == e.name.rsplit('.').next().unwrap_or("")
        })
        .is_some_and(|e| e.raw);
    let (input, output) = op_io(op);
    let bodies: Vec<String> = answers
        .iter()
        .map(|answer| {
            if raw {
                refs.push(Symbol::imported("tonoext", EXT_RUNTIME_MODULE, "tonoext"));
                raw_answer_body(ctx, answer, refs)
            } else {
                typed_answer_body(ctx, output, answer, refs)
            }
        })
        .collect();
    let body = if bodies.len() == 1 {
        bodies.into_iter().next().unwrap_or_default()
    } else {
        let mut arms = String::new();
        for (i, b) in bodies.iter().enumerate() {
            if i + 1 == bodies.len() {
                arms.push_str(&format!("\t\tdefault:\n{}", indent_go(b)));
            } else {
                arms.push_str(&format!("\t\tcase {i}:\n{}", indent_go(b)));
            }
        }
        format!("\t\ti := implCalls\n\t\timplCalls++\n\t\tswitch i {{\n{arms}\t\t}}\n")
    };
    let sig = if raw {
        format!(
            "func(ctx context.Context, s *{settings}, payload []byte) (tonoext.Outcome, error)",
            settings = ctx.n.settings
        )
    } else {
        let param = match input {
            Some(t) => {
                push_type_symbols(t, refs);
                format!(", input {}", go_type(t))
            }
            None => String::new(),
        };
        let ret = match output {
            Some(t) => {
                push_type_symbols(t, refs);
                format!("({}, error)", go_type(t))
            }
            None => "error".to_string(),
        };
        format!(
            "func(ctx context.Context, s *{settings}{param}) {ret}",
            settings = ctx.n.settings
        )
    };
    let counter = if answers.len() > 1 {
        "\timplCalls := 0\n"
    } else {
        ""
    };
    format!(
        "\tprev := {seam}\n\
         {counter}\
         \t{seam} = {sig} {{\n{body}\t}}\n\
         \tdefer func() {{ {seam} = prev }}()\n"
    )
}

/// The canned body of one typed-impl answer: return the value, the typed
/// declared error, or an undeclared failure the glue wraps into a contract
/// error.
fn typed_answer_body(
    ctx: &TestCtx<'_>,
    output: Option<&Tref>,
    answer: &StubAnswer,
    refs: &mut Vec<Symbol>,
) -> String {
    let zero = match output {
        Some(t) => {
            push_type_symbols(t, refs);
            format!("\t\tvar zero {}\n", go_type(t))
        }
        None => String::new(),
    };
    let ret = |err: String| match output {
        Some(_) => format!("{zero}\t\treturn zero, {err}\n"),
        None => format!("\t\treturn {err}\n"),
    };
    match answer {
        StubAnswer::Value { value } => match output {
            Some(t) => {
                refs.push(import("json", "encoding/json"));
                format!(
                    "\t\tvar out {ty}\n\
                     \t\tif err := json.Unmarshal([]byte({raw}), &out); err != nil {{\n\
                     \t\t\tt.Fatalf(\"decode declared value: %v\", err)\n\t\t}}\n\
                     \t\treturn out, nil\n",
                    ty = go_type(t),
                    raw = go_string(&json_text(value)),
                )
            }
            None => "\t\treturn nil\n".to_string(),
        },
        StubAnswer::Error { error } => ret(declared_error_literal(ctx, &error.shape, &error.data)),
        StubAnswer::Contract { .. } => {
            refs.push(import("errors", "errors"));
            ret("errors.New(\"simulated bespoke failure\")".to_string())
        }
        // Rejected by validation: an http answer never reaches an impl stub.
        StubAnswer::Http(_) => ret("errors.New(\"unsupported stub answer\")".to_string()),
    }
}

/// The canned raw outcome: a value carries the wire body, an error carries the
/// code the glue discriminates on, a contract answer fails outright.
fn raw_answer_body(ctx: &TestCtx<'_>, answer: &StubAnswer, refs: &mut Vec<Symbol>) -> String {
    match answer {
        StubAnswer::Value { value } => format!(
            "\t\treturn tonoext.Outcome{{Success: true, Body: []byte({})}}, nil\n",
            go_string(&json_text(value)),
        ),
        StubAnswer::Error { error } => {
            let op = ctx.test.op.expect("an impl stub rides a call");
            let code = declared_tests::declared_error_by_shape(op, ctx.module, &error.shape)
                .and_then(|e| e.code)
                .map(|c| c.value)
                .unwrap_or_default();
            format!(
                "\t\treturn tonoext.Outcome{{Success: false, Code: {code:?}, Body: []byte({body})}}, nil\n",
                body = go_string(&json_text(&error.data)),
            )
        }
        StubAnswer::Contract { .. } => {
            refs.push(import("errors", "errors"));
            "\t\treturn tonoext.Outcome{}, errors.New(\"simulated bespoke failure\")\n".to_string()
        }
        // Rejected by validation: an http answer never reaches an impl stub.
        StubAnswer::Http(_) => "\t\treturn tonoext.Outcome{}, nil\n".to_string(),
    }
}

/// The typed literal of a declared error named by its shape, its fields filled
/// from the answer's wire data. An impl stub's error is one of the called
/// op's own declared errors; a handle-method stub's error is a shape a
/// sentinel of the method's `errors:` maps to (resolved by local name, the
/// way `ext::declared_error_literal` resolves it), which may be answered in a
/// construction-only test with no op at all.
fn declared_error_literal(ctx: &TestCtx<'_>, shape: &str, data: &serde_json::Value) -> String {
    let en = error_names();
    let shape_id = ctx
        .test
        .op
        .and_then(|op| declared_tests::declared_error_by_shape(op, ctx.module, shape))
        .map(|err| err.shape_id)
        .or_else(|| {
            ctx.module
                .shapes
                .iter()
                .find(|s| crate::codegen::entries::local_name(&s.id) == shape)
                .map(|s| s.id.clone())
        });
    let Some(shape_id) = shape_id else {
        // Unreachable when the tests passed validation.
        return format!("&{api}{{}}", api = en.api);
    };
    let ty = crate::codegen::conventions::type_ident_from_id(&shape_id);
    let members = shape_members(ctx.module, &shape_id);
    let fields: String = data
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(key, value)| {
                    let member = members
                        .iter()
                        .find(|m| crate::codegen::conventions::wire_key(m) == *key)?;
                    Some(format!(
                        "{}: {}, ",
                        super::field_pascal(&member.name, ctx.config),
                        pinned_literal(ctx.module, ctx.config, &member.target, value)
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    format!("&{ty}{{{fields}}}")
}

fn shape_members(module: &Module, shape_id: &str) -> Vec<crate::ir::Member> {
    module
        .shapes
        .iter()
        .find(|s| s.id == shape_id)
        .map(|s| match &s.kind {
            crate::ir::ShapeKind::Structure { members, .. } => members.clone(),
            _ => Vec::new(),
        })
        .unwrap_or_default()
}

/// The invocation of the generated method: decode the wire input, call, bind
/// the outcome the expectation needs.
fn invoke_block(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    let call = ctx.test.call.expect("invocation needs a call");
    let op = ctx.test.op.expect("a call resolved its op");
    let method = method_name(op, ctx.config);
    let (input, output) = op_io(op);
    refs.push(import("context", "context"));
    let mut text = String::new();
    let call_input = match (input, &call.input) {
        (Some(t), Some(value)) => {
            push_type_symbols(t, refs);
            refs.push(import("json", "encoding/json"));
            text.push_str(&format!(
                "\tvar input {ty}\n\
                 \tif err := json.Unmarshal([]byte({raw}), &input); err != nil {{\n\
                 \t\tt.Fatalf(\"decode declared input: %v\", err)\n\t}}\n",
                ty = go_type(t),
                raw = go_string(&json_text(value)),
            ));
            ", input"
        }
        _ => "",
    };
    // `err` is already bound by construction, so only a binding introducing a
    // new variable may use `:=`.
    let wants_out = output.is_some()
        && matches!(
            ctx.test.outcome,
            Some(TestPattern::Eq(_) | TestPattern::Struct(_))
        );
    let bind = match (output, wants_out) {
        (Some(_), true) => "out, err := ",
        (Some(_), false) => "_, err = ",
        (None, _) => "err = ",
    };
    text.push_str(&format!(
        "\t{bind}c.{method}(context.Background(){call_input})\n"
    ));
    text
}

/// One hermetic test: pin the environment, install the declared stubs, build
/// the client through the same three steps the constructor runs (the shared
/// settings step, the foreign constructions, the client), with a fake in
/// place of every stubbed construction, then run the call and assert. A
/// test with no construction stub at all builds through `New` itself; a
/// construction-only test just constructs and asserts its outcome.
fn hermetic_test_decl(ctx: &TestCtx<'_>, first_unset: &mut bool) -> Vec<Decl> {
    let mut refs = vec![import("testing", "testing")];
    let mut body = String::new();
    let mut extra_decls = Vec::new();
    body.push_str(&env_pinning(ctx, &mut refs, first_unset));
    let (args, opts) = construction_args(ctx, &mut refs);
    let http_stub = ctx.test.stub.filter(|stub| stub.dep == StubDep::Http);
    if let Some(stub) = http_stub {
        refs.push(support_symbol("HTTPRequest"));
        refs.push(support_symbol("HTTPResponse"));
        refs.push(import("context", "context"));
        let answers: Vec<&HttpAnswer> = stub
            .answers
            .iter()
            .filter_map(|a| match a {
                StubAnswer::Http(h) => Some(h),
                _ => None,
            })
            .collect();
        body.push_str(&transport_block(&answers));
    }
    if let Some(stub) = ctx.test.stub.filter(|stub| stub.dep == StubDep::Impl) {
        body.push_str(&impl_stub_block(ctx, &stub.answers, &mut refs));
    }
    let assembled = http_stub.is_some() || !ctx.test.extern_stubs.is_empty();
    let construct_args = join_args(&args, &[&opts]);
    let bind = if ctx.test.call.is_some() { "c" } else { "_" };
    let construct = if assembled {
        let (pre, steps) = assembly_steps(ctx, &mut refs, &mut extra_decls);
        body.push_str(&pre);
        let transport = if http_stub.is_some() {
            "\t\ts.Transport = transport\n"
        } else {
            ""
        };
        format!(
            "\tbuild := func() (*{client}, error) {{\n\
             \t\ts, err := {settings_fn}({construct_args})\n\
             \t\tif err != nil {{\n\t\t\treturn nil, err\n\t\t}}\n\
             {steps}{transport}\
             \t\treturn {client_fn}(s)\n\
             \t}}\n\
             \t{bind}, err := build()\n",
            client = ctx.n.client,
            settings_fn = super::constructor::settings_fn_name(ctx.n),
            client_fn = super::constructor::client_fn_name(ctx.n),
        )
    } else {
        format!(
            "\t{bind}, err := {new_fn}({construct_args})\n",
            new_fn = ctx.n.new_fn,
        )
    };
    body.push_str(&construct);
    if ctx.test.call.is_some() {
        body.push_str("\tif err != nil {\n\t\tt.Fatalf(\"construct client: %v\", err)\n\t}\n");
        body.push_str(&invoke_block(ctx, &mut refs));
        body.push_str(&outcome_asserts(ctx, &mut refs));
        if let Some(patterns) = ctx.test.requests {
            body.push_str(&request_asserts(patterns, &mut refs));
        }
    } else {
        // Construction-only: the outcome pattern reads the construction error.
        body.push_str(&outcome_asserts(ctx, &mut refs));
    }
    let mut decls = vec![Decl::raw_with(
        format!(
            "func {name}(t *testing.T) {{\n{body}}}",
            name = test_fn_name(ctx),
        ),
        refs,
    )];
    decls.extend(extra_decls);
    decls
}

#[path = "vector_extern.rs"]
mod vector_extern;
use vector_extern::assembly_steps;
#[path = "vector_values.rs"]
mod values;
use values::{pinned_literal, zero_literal};

/// One live test: no stub and no pinned environment; construction reads the
/// ambient env (real credentials), and the same expectations verify that the
/// spec still matches the real dependency.
fn live_test_decl(ctx: &TestCtx<'_>) -> Decl {
    let mut refs = vec![import("testing", "testing"), import("context", "context")];
    let mut body = String::new();
    let (args, opts) = construction_args(ctx, &mut refs);
    body.push_str(&format!(
        "\tc, err := {new_fn}({call})\n\
         \tif err != nil {{\n\t\tt.Fatalf(\"construct client: %v\", err)\n\t}}\n",
        new_fn = ctx.n.new_fn,
        call = join_args(&args, &[&opts]),
    ));
    body.push_str(&invoke_block(ctx, &mut refs));
    body.push_str(&outcome_asserts(ctx, &mut refs));
    Decl::raw_with(
        format!(
            "func {name}Live(t *testing.T) {{\n{body}}}",
            name = test_fn_name(ctx),
        ),
        refs,
    )
}

#[cfg(test)]
#[path = "vector_expects_tests.rs"]
mod expects_tests;

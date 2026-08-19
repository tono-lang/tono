//! Native `cargo test` files generated from the module's declared tests.
//!
//! Each test runs the real client method through the real construction path;
//! an `.http` stub answers through the constructor's seam
//! (`new_with_transport`/`build_with_transport`), so the test sees exactly
//! the request the SDK would send. A test whose call has no stub runs against
//! the real dependency, so it lands in the live file and every live test
//! carries `#[ignore]` to stay out of a default `cargo test` run. A test that
//! stubs an `.impl` dependency generates nothing here: the Rust bespoke ops
//! expose no swappable per-operation seam, so only the transport stub and the
//! live path can be exercised natively. The same holds for a call whose own
//! dependency is neither `.http` nor `.impl`: an extern handle method reached
//! through the op's own `impl` body (hermetic on its `extern_stubs` coverage
//! alone, with no call-scoped stub). This target emits that call but exposes
//! no seam a test could swap the handle method through, so such a test also
//! generates nothing here rather than a "hermetic" test that reaches the
//! real library.
//!
//! Each generated file is a `#[cfg(test)]` module of the SDK crate itself
//! (the module tree declares it), which is what lets it reach the
//! `pub(crate)` construction seam while the shipped surface stays clean.

use super::*;
use crate::codegen::declared_tests::{self, PlannedTest};

#[path = "vector_expects.rs"]
mod expects;
use crate::codegen::group::Group;
use crate::codegen::tree::ModuleFile;
use crate::ir::{EnvName, HttpAnswer, StubAnswer, StubDep, TestStub};
use expects::{outcome_asserts, request_asserts};

const BINDING_LANGS: [&str; 1] = ["rust"];

/// The generated test files of a module's entries: one hermetic and one live
/// file per entry that declares tests, and nothing at all for one that has
/// none (or whose every hermetic test stubs an impl or rides only on extern
/// handle-method stubs, which this target skips).
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
        for test in &group.tests {
            let ctx = TestCtx {
                entry,
                n: &n,
                module,
                config,
                multi,
                test,
            };
            if !test.hermetic {
                live.push(live_test_decl(&ctx));
                continue;
            }
            // An impl-stubbed test is skipped for Rust (see module doc), and
            // so is a call hermetic only through its extern handle-method
            // stubs (`hermetic_test_decl` yields nothing for it).
            if test.stub.is_some_and(|s| s.dep == StubDep::Impl) {
                continue;
            }
            hermetic.extend(hermetic_test_decl(&ctx));
        }
        if !hermetic.is_empty() {
            let mut decls = vec![
                hermetic_doc(),
                glob_use(&Group::types(&module.name)),
                glob_use(&Group::entry(&module.name, entry.name)),
                env_lock_decl(),
            ];
            decls.extend(hermetic);
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, false),
                decls,
            ));
        }
        if !live.is_empty() {
            let mut decls = vec![
                live_doc(),
                glob_use(&Group::types(&module.name)),
                glob_use(&Group::entry(&module.name, entry.name)),
            ];
            decls.extend(live);
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, true),
                decls,
            ));
        }
    }
    files
}

/// Everything one test's function needs from its surroundings.
struct TestCtx<'a> {
    entry: &'a EntryModel<'a>,
    n: &'a Names,
    module: &'a Module,
    config: &'a CasingConfig,
    multi: bool,
    test: &'a PlannedTest<'a>,
}

impl TestCtx<'_> {
    fn values(&self) -> &std::collections::BTreeMap<String, serde_json::Value> {
        &self.test.construction.values
    }
}

// Plain `//` comments, not `//!` module docs: a test file with an http stub
// collects real imports (the transport support types), and the rendered
// `use` section lands above the first declaration, where an inner doc
// comment would no longer be at the top of the file (E0753).
fn hermetic_doc() -> Decl {
    Decl::raw(
        "// Generated from the entry's declared tests: each one runs the real\n\
         // construction path and the real method, with only the stubbed\n\
         // transport swapped through the constructor's seam. Impl-stubbed tests\n\
         // and tests riding only on extern handle-method stubs generate nothing\n\
         // for Rust: neither has a swappable seam here."
            .to_string(),
    )
}

fn live_doc() -> Decl {
    Decl::raw(
        "// The live tests of the entry's declared tests: no stub, so\n\
         // construction reads the ambient environment (real credentials) and\n\
         // every test is ignored by default; run `cargo test -- --ignored` to\n\
         // exercise the real dependency."
            .to_string(),
    )
}

/// A glob `use` of a group of the entry's own module: the tests name the
/// client, the wire types, and the error taxonomy as bare identifiers, the
/// same technique the entry group itself uses to reach the taxonomy.
fn glob_use(group: &Group) -> Decl {
    crate::codegen::targets::rust::emit::types_glob_use(group)
}

fn env_lock_decl() -> Decl {
    Decl::raw(
        "// Environment pinning writes process-wide state, so every pinned test\n\
         // serializes on this lock; a poisoned lock still hands the guard over,\n\
         // so one failing test never wedges the rest of the run.\n\
         static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());"
            .to_string(),
    )
}

/// `save_note_the_glue_guards`: the called operation (when there is one) plus
/// the test name, each reduced to Rust identifier characters. Live tests carry
/// a `_live` suffix so a test name can never produce the same function in
/// both files.
fn test_fn_name(test: &PlannedTest<'_>, live: bool) -> String {
    let words: Vec<String> = test
        .name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    let mut name = match test.op {
        Some(op) => declared_tests::bare_op_name(&op.id).to_string(),
        None => "construction".to_string(),
    };
    if !words.is_empty() {
        name.push('_');
        name.push_str(&words.join("_"));
    }
    if live {
        name.push_str("_live");
    }
    name
}

/// A Rust string literal carrying arbitrary text: a raw string when the text
/// cannot close it early, the escaped spelling otherwise.
fn rust_string(text: &str) -> String {
    if text.contains("\"#") {
        format!("{text:?}")
    } else {
        format!("r#\"{text}\"#")
    }
}

fn json_text(value: &serde_json::Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".into())
}

/// The string an env var is set to: a JSON string verbatim, anything else in
/// its JSON spelling (env values are text by nature).
fn env_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => json_text(other),
    }
}

/// The environment pinning of a hermetic test: every `@env` name a pinned
/// construction value maps to is set, and every other literal env name the
/// entry could read is cleared, so the test resolves the same values on any
/// machine. Edition 2021, so `set_var`/`remove_var` are safe; the file-level
/// `ENV_LOCK` serializes the writes.
fn env_pinning(ctx: &TestCtx<'_>) -> String {
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
                    let value = env_value(value);
                    lines.push_str(&format!("    std::env::set_var({name:?}, {value:?});\n"));
                    pinned = true;
                }
                _ => lines.push_str(&format!("    std::env::remove_var({name:?});\n")),
            }
        }
    }
    lines
}

/// The `.with_*` method name of a `@with` field, exactly as the builder
/// spells it (see `constructor::construction_decls`).
fn with_fn_name(ctx: &TestCtx<'_>, f: &EntryField) -> String {
    let display = rename_of(&f.traits, LANG).unwrap_or_else(|| f.name.clone());
    snake(&format!(
        "with_{}",
        companion_name(ctx.entry.name, &display, ctx.multi)
    ))
}

/// Whether this entry's construction itself is async: true whenever a
/// declared field resolves through an `extern` call, which `constructor.rs`
/// already lowers to an `async fn new`/`new_with_transport` (an arbitrary
/// third-party symbol is always awaited, mirroring every other async-lowered
/// leaf this target emits).
fn construction_is_async(ctx: &TestCtx<'_>) -> bool {
    ctx.entry.declared().iter().any(|f| f.call.is_some())
}

/// The construction expression of one test, from the pinned construction
/// values: `@arg` fields positionally, `@with` fields through the builder. A
/// stubbed test goes through the transport seam; anything else through the
/// public constructor. An unpinned `@arg` gets the type's zero value: its
/// declared chain has nothing else to resolve from, and the zero value keeps
/// a construction-failure expectation expressible.
fn construction_expr(ctx: &TestCtx<'_>, transport: bool) -> String {
    let args: Vec<String> = ctx
        .entry
        .args()
        .iter()
        .map(|f| match ctx.values().get(&f.name) {
            Some(v) => super::literal(&f.target, v, ctx.module),
            None => super::literal(
                &f.target,
                &serde_json::Value::String(String::new()),
                ctx.module,
            ),
        })
        .collect();
    let client = &ctx.n.client;
    if ctx.entry.with_fields().is_empty() {
        if transport {
            let mut call = String::from("Some(transport)");
            for a in &args {
                call.push_str(", ");
                call.push_str(a);
            }
            format!("{client}::new_with_transport({call})")
        } else {
            format!("{client}::new({})", args.join(", "))
        }
    } else {
        let with_calls: String = ctx
            .entry
            .with_fields()
            .iter()
            .filter_map(|f| {
                let v = ctx.values().get(&f.name)?;
                Some(format!(
                    ".{}({})",
                    with_fn_name(ctx, f),
                    super::literal(&f.target, v, ctx.module)
                ))
            })
            .collect();
        let build = if transport {
            "build_with_transport(Some(transport))"
        } else {
            "build()"
        };
        format!("{client}::builder({}){with_calls}.{build}", args.join(", "))
    }
}

/// The `.http` stub: the canned responses answered per call (the last one
/// repeats, which is how `@retry` resilience is exercised), with every
/// request recorded for the `requests` expectations. A single answer returns
/// directly, with no sequence index.
fn transport_block(answers: &[&HttpAnswer]) -> String {
    let literal = |a: &HttpAnswer, indent: &str| {
        let headers = if a.headers.is_empty() {
            "std::collections::HashMap::new()".to_string()
        } else {
            let pairs: Vec<String> = a
                .headers
                .iter()
                .map(|(k, v)| format!("({k:?}.to_string(), {v:?}.to_string())"))
                .collect();
            format!("std::collections::HashMap::from([{}])", pairs.join(", "))
        };
        format!(
            "HttpResponse {{\n\
             {indent}    status: {status},\n\
             {indent}    headers: {headers},\n\
             {indent}    body: {body}.to_string(),\n\
             {indent}}}",
            status = a.status,
            body = rust_string(&a.body),
        )
    };
    let record_and_answer = if answers.len() == 1 {
        format!(
            "                recorded.lock().unwrap_or_else(|e| e.into_inner()).push(req);\n\
             \x20               Ok({})\n",
            literal(answers[0], "                ")
        )
    } else {
        let mut arms = String::new();
        for (at, a) in answers.iter().enumerate() {
            let pat = if at + 1 == answers.len() {
                "_".to_string()
            } else {
                at.to_string()
            };
            arms.push_str(&format!(
                "                    {pat} => {},\n",
                literal(a, "                    ")
            ));
        }
        format!(
            "                let i = {{\n\
             \x20                   let mut seen = recorded.lock().unwrap_or_else(|e| e.into_inner());\n\
             \x20                   seen.push(req);\n\
             \x20                   seen.len() - 1\n\
             \x20               }};\n\
             \x20               Ok(match i {{\n{arms}                }})\n"
        )
    };
    format!(
        "    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<HttpRequest>::new()));\n\
         \x20   let recorded = seen.clone();\n\
         \x20   let transport: HttpTransport =\n\
         \x20       std::sync::Arc::new(move |req: HttpRequest| {{\n\
         \x20           let recorded = recorded.clone();\n\
         \x20           Box::pin(async move {{\n\
         {record_and_answer}\
         \x20           }})\n\
         \x20       }});\n"
    )
}

/// The invocation of the generated method: decode the wire input, call, bind
/// the outcome as `result`.
fn invoke_block(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    let call = ctx.test.call.expect("invocation needs a call");
    let op = ctx.test.op.expect("a call resolved its op");
    let method = surface::method_name(op, ctx.config);
    let (input, _) = op_io(op);
    let is_async = wire_binding(op).is_some() || effect_of(op) == Effect::Async;
    let mut text = String::new();
    let call_input = match (input, &call.input) {
        (Some(t), Some(value)) => {
            push_type_symbols(t, &ctx.module.name, refs);
            text.push_str(&format!(
                "    let input: {ty} = serde_json::from_str({raw}).expect(\"decode declared input\");\n",
                ty = rust_type(t),
                raw = rust_string(&json_text(value)),
            ));
            "input"
        }
        _ => "",
    };
    let awaited = if is_async { ".await" } else { "" };
    text.push_str(&format!(
        "    let result = c.{method}({call_input}){awaited};\n"
    ));
    text
}

/// One hermetic test: serialize on the env lock, pin the environment, stub
/// the transport, build the client through the real construction path, run
/// the call, assert. A construction-only test just constructs and asserts its
/// outcome; it stays synchronous unless construction itself is async (an
/// `extern`-call field). A stubbed transport only attaches to an `@http`
/// operation, so a stubbed call is always async and rides the tokio runtime
/// the consuming crate's dev profile already carries. `None` for a call with
/// no call-scoped stub (hermetic only through extern handle-method stubs, see
/// the module doc): nothing here can stand in for the handle method.
fn hermetic_test_decl(ctx: &TestCtx<'_>) -> Option<Decl> {
    let mut refs = Vec::new();
    let mut body =
        String::from("    let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());\n");
    body.push_str(&env_pinning(ctx));
    let (attr, effect) = if ctx.test.call.is_some() {
        let stub = ctx.test.stub?;
        body.push_str(&stubbed_call_body(ctx, stub, &mut refs));
        ("#[tokio::test]", "async ")
    } else {
        // Construction-only: the outcome pattern reads the construction
        // error. Synchronous unless an `extern`-call field makes
        // construction itself async, in which case the test rides the same
        // tokio runtime the stubbed-call branch above does.
        let is_async = construction_is_async(ctx);
        let await_ = if is_async { ".await" } else { "" };
        body.push_str(&format!(
            "    let result = {expr}{await_};\n",
            expr = construction_expr(ctx, false),
        ));
        body.push_str(&outcome_asserts(ctx, false));
        if is_async {
            ("#[tokio::test]", "async ")
        } else {
            ("#[test]", "")
        }
    };
    Some(Decl::raw_with(
        format!(
            "{attr}\n{effect}fn {name}() {{\n{body}}}",
            name = test_fn_name(ctx.test, false),
        ),
        refs,
    ))
}

/// The body of a stubbed call: the transport stub, the client built through
/// the seam, the invocation, and the outcome/request assertions.
fn stubbed_call_body(ctx: &TestCtx<'_>, stub: &TestStub, refs: &mut Vec<Symbol>) -> String {
    let answers: Vec<&HttpAnswer> = stub
        .answers
        .iter()
        .filter_map(|a| match a {
            StubAnswer::Http(h) => Some(h),
            _ => None,
        })
        .collect();
    refs.push(super::support_symbol("HttpRequest"));
    refs.push(super::support_symbol("HttpResponse"));
    refs.push(super::support_symbol("HttpTransport"));
    let mut body = transport_block(&answers);
    let await_ = if construction_is_async(ctx) {
        ".await"
    } else {
        ""
    };
    body.push_str(&format!(
        "    let c = {expr}{await_}.expect(\"construct client\");\n",
        expr = construction_expr(ctx, true),
    ));
    body.push_str(&invoke_block(ctx, refs));
    let has_output = ctx.test.op.and_then(|op| op_io(op).1).is_some();
    body.push_str(&outcome_asserts(ctx, has_output));
    if let Some(patterns) = ctx.test.requests {
        body.push_str(&request_asserts(patterns));
    }
    body
}

/// One live test: no stub and no pinned environment; construction reads the
/// ambient env (real credentials), and the same expectations verify that the
/// spec still matches the real dependency.
fn live_test_decl(ctx: &TestCtx<'_>) -> Decl {
    let mut refs = Vec::new();
    let op = ctx.test.op.expect("a live test has a call");
    let is_async =
        wire_binding(op).is_some() || effect_of(op) == Effect::Async || construction_is_async(ctx);
    let (attr, effect) = if is_async {
        ("#[tokio::test]", "async ")
    } else {
        ("#[test]", "")
    };
    let construct_await = if construction_is_async(ctx) {
        ".await"
    } else {
        ""
    };
    let mut body = format!(
        "    let c = {expr}{construct_await}.expect(\"construct client\");\n",
        expr = construction_expr(ctx, false),
    );
    body.push_str(&invoke_block(ctx, &mut refs));
    let has_output = op_io(op).1.is_some();
    body.push_str(&outcome_asserts(ctx, has_output));
    Decl::raw_with(
        format!(
            "{attr}\n#[ignore = \"runs against the live endpoint; pass --ignored to include\"]\n{effect}fn {name}() {{\n{body}}}",
            name = test_fn_name(ctx.test, true),
        ),
        refs,
    )
}

#[cfg(test)]
#[path = "vector_expects_tests.rs"]
mod expects_tests;

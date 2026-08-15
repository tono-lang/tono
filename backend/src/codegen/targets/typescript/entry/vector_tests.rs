//! Native Vitest files generated from the module's declared tests.
//!
//! Each test runs the real client method through the real construction path;
//! only the declared stub point is swapped: an `.http` stub answers through
//! the class seam (`Client.forTest`), an `.impl` stub goes through the
//! exported per-operation swapper ([`super::impl_op::swap_fn_name`]). A test
//! whose call has no stub runs against the real dependency, so it lands in the
//! live file, gated behind `TONO_LIVE_TESTS=1` so it stays out of a default
//! `vitest` run; a construction-only test is hermetic by nature. A call whose
//! own dependency is neither `.http` nor `.impl` can only be an extern handle
//! method reached through the op's own `impl` body, which generation-time
//! validation ([`TargetKind::emits_ext_handle_calls`]) already refuses for
//! this target before any test file is built, so that combination never
//! reaches this emitter.
//!
//! Unlike Go's shared package scope, each test file is its own ES module, so
//! it imports the surface it exercises. Each test body is self-contained
//! straight-line code; the only file-level declaration is the `vectorEnv`
//! value binding (the ambient process env).

use std::collections::BTreeMap;

use crate::codegen::casing::CasingConfig;
use crate::codegen::conventions::{rename_of, type_ident_from_id};
use crate::codegen::declared_tests::{self, PlannedTest};
use crate::codegen::entries::{plan, EntryModel};
use crate::codegen::extensions::{impl_binding, BoundExtension};
use crate::codegen::group::Group;
use crate::codegen::ops::op_io;
use crate::codegen::symbol::Symbol;
use crate::codegen::targets::typescript::codecs::decode_expr;
use crate::codegen::targets::typescript::types::LANG;
use crate::codegen::tree::{Decl, ModuleFile};
use crate::ir::{EnvName, HttpAnswer, Module, Source, StubAnswer, StubDep, TestPattern, Tref};

use super::{field_camel_ren, literal, module_symbol, names, support_symbol, Names};

#[path = "vector_expects.rs"]
mod expects;
use expects::{eq_assert, failure_asserts, request_asserts, struct_asserts};

const BINDING_LANGS: [&str; 2] = ["ts", "typescript"];

/// The generated test files of a module's entries: one hermetic and one live
/// file per entry that declares tests, and nothing at all for one that has
/// none.
pub(crate) fn test_files(module: &Module, config: &CasingConfig) -> Vec<ModuleFile> {
    let Some((entries, multi, bound)) = plan::entry_setup(module, &BINDING_LANGS) else {
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
        let n = names(entry, multi);
        let mut hermetic_cases = Vec::new();
        let mut live_cases = Vec::new();
        let mut hermetic_refs = vitest_refs();
        let mut live_refs = vitest_refs();
        let mut hermetic_uses_env = false;
        for test in &group.tests {
            let ctx = TestCtx {
                entry,
                n: &n,
                module,
                config,
                bound: &bound,
                test,
            };
            if test.hermetic {
                hermetic_cases.push(indent(
                    &hermetic_case(&ctx, &mut hermetic_uses_env, &mut hermetic_refs),
                    "  ",
                ));
            } else {
                live_cases.push(indent(&live_case(&ctx, &mut live_refs), "  "));
            }
        }
        // The env const precedes the cases: a `const` does not hoist, and a
        // describe argument evaluates at module load.
        if !hermetic_cases.is_empty() {
            let mut decls = Vec::new();
            if hermetic_uses_env {
                decls.push(env_const_decl());
            }
            decls.push(Decl::raw_with(
                format!(
                    "describe({name}, () => {{\n{body}}});",
                    name = ts_str(entry.name),
                    body = hermetic_cases.join("\n"),
                ),
                hermetic_refs,
            ));
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, false),
                decls,
            ));
        }
        if !live_cases.is_empty() {
            // The live gate reads the ambient env, so its const always rides.
            let mut decls = vec![env_const_decl()];
            decls.push(Decl::raw_with(
                format!(
                    "// The live cases run against the real dependency (no stub, no pinned\n\
                     // environment); opt in with TONO_LIVE_TESTS=1.\n\
                     describe.runIf(vectorEnv[\"TONO_LIVE_TESTS\"] === \"1\")({name}, () => {{\n{body}}});",
                    name = ts_str(&format!("{} (live)", entry.name)),
                    body = live_cases.join("\n"),
                ),
                live_refs,
            ));
            files.push(ModuleFile::new(
                Group::tests(&module.name, entry.name, true),
                decls,
            ));
        }
    }
    files
}

/// Everything one test's body needs from its surroundings.
struct TestCtx<'a> {
    entry: &'a EntryModel<'a>,
    n: &'a Names,
    module: &'a Module,
    config: &'a CasingConfig,
    bound: &'a [BoundExtension<'a>],
    test: &'a PlannedTest<'a>,
}

impl TestCtx<'_> {
    fn values(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.test.construction.values
    }
}

fn vitest_refs() -> Vec<Symbol> {
    ["describe", "expect", "it"]
        .iter()
        .map(|name| Symbol::imported(*name, "vitest", *name))
        .collect()
}

/// A TypeScript string literal carrying arbitrary text (JSON escaping is a
/// valid double-quoted TypeScript spelling).
fn ts_str(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".into())
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

fn indent(block: &str, prefix: &str) -> String {
    block
        .lines()
        .map(|line| {
            if line.is_empty() {
                "\n".to_string()
            } else {
                format!("{prefix}{line}\n")
            }
        })
        .collect()
}

/// The environment pinning of a hermetic test: every `@env` name a pinned
/// construction value maps to is set, and every other literal env name the
/// entry could read is cleared, so the test resolves the same values on any
/// machine. The pins are deduplicated (an object literal refuses a repeated
/// key); a pin wins over a clear for a variable two chains share.
fn env_pins(ctx: &TestCtx<'_>) -> Vec<(String, Option<String>)> {
    let mut pins: Vec<(String, Option<String>)> = Vec::new();
    for field in &ctx.entry.fields {
        let covered = ctx.values().get(&field.name);
        let mut pinned = false;
        for source in &field.sources {
            let Source::Env(EnvName::Name(name)) = source else {
                continue;
            };
            let value = match covered {
                Some(value) if !pinned => {
                    pinned = true;
                    Some(env_value(value))
                }
                _ => None,
            };
            match pins.iter_mut().find(|(existing, _)| existing == name) {
                Some(existing) => {
                    if existing.1.is_none() {
                        existing.1 = value;
                    }
                }
                None => pins.push((name.clone(), value)),
            }
        }
    }
    pins
}

/// The constructor arguments a test passes, from the pinned construction
/// values: `@arg` fields positionally, covered `@with` fields as the trailing
/// config object. An unpinned `@arg` gets the type's zero value: its declared
/// chain has nothing else to resolve from, and the zero value keeps a
/// construction-failure expectation expressible.
fn construction_args(ctx: &TestCtx<'_>) -> String {
    let mut parts: Vec<String> = ctx
        .entry
        .args()
        .iter()
        .map(|f| match ctx.values().get(&f.name) {
            Some(v) => literal(&f.target, v),
            None => literal(&f.target, &serde_json::Value::String(String::new())),
        })
        .collect();
    let with: Vec<String> = ctx
        .entry
        .with_fields()
        .iter()
        .filter_map(|f| {
            let v = ctx.values().get(&f.name)?;
            Some(format!(
                "{}: {}",
                field_camel_ren(&f.name, rename_of(&f.traits, LANG).as_deref(), ctx.config),
                literal(&f.target, v)
            ))
        })
        .collect();
    if !with.is_empty() {
        parts.push(format!("{{ {} }}", with.join(", ")));
    }
    parts.join(", ")
}

/// The `.http` stub: the canned responses answered per call (the last one
/// repeats, which is how `@retry` resilience is exercised), with every request
/// recorded for the `requests` expectations. A single answer returns directly,
/// with no sequence machinery.
fn transport_block(answers: &[&HttpAnswer]) -> String {
    let literal = |a: &&HttpAnswer| {
        let headers = if a.headers.is_empty() {
            "{}".to_string()
        } else {
            let entries: Vec<String> = a
                .headers
                .iter()
                .map(|(k, v)| format!("{}: {}", ts_str(k), ts_str(v)))
                .collect();
            format!("{{ {} }}", entries.join(", "))
        };
        format!(
            "{{ status: {status}, headers: {headers}, body: {body} }}",
            status = a.status,
            body = ts_str(&a.body),
        )
    };
    match answers {
        [only] => format!(
            "const seen: HttpRequest[] = [];\n\
             const transport: HttpTransport = async (req) => {{\n\
             \x20 seen.push(req);\n\
             \x20 return {response};\n\
             }};\n",
            response = literal(only),
        ),
        _ => {
            let responses: Vec<String> = answers
                .iter()
                .map(|a| format!("  {},\n", literal(a)))
                .collect();
            format!(
                "const seen: HttpRequest[] = [];\n\
                 const responses = [\n{responses}];\n\
                 const transport: HttpTransport = async (req) => {{\n\
                 \x20 seen.push(req);\n\
                 \x20 return responses[Math.min(seen.length - 1, responses.length - 1)];\n\
                 }};\n",
                responses = responses.join(""),
            )
        }
    }
}

/// The expression decoding a declared value into the in-memory type, through
/// the same wire codecs the generated glue uses.
fn decoded_value_expr(
    t: &Tref,
    value: &serde_json::Value,
    refs: &mut Vec<Symbol>,
    module: &Module,
) -> String {
    if let Tref::Ref { id, .. } = t {
        refs.push(module_symbol(
            &format!("decode{}", type_ident_from_id(id)),
            module,
        ));
    }
    decode_expr(&json_text(value), t)
}

/// The canned body of one impl-stub answer (the arrow the swapper installs).
fn answer_body(ctx: &TestCtx<'_>, answer: &StubAnswer, refs: &mut Vec<Symbol>) -> String {
    let op = ctx.test.op.expect("an impl stub rides a call");
    let raw = impl_binding(ctx.bound, &op.id).is_some_and(|b| b.raw);
    if raw {
        return raw_answer_body(ctx, answer);
    }
    let (_, output) = op_io(op);
    typed_answer_body(ctx, output, answer, refs)
}

/// The canned body of one typed-impl answer: return the value, throw the
/// typed declared error, or throw an undeclared failure the glue wraps into a
/// contract error.
fn typed_answer_body(
    ctx: &TestCtx<'_>,
    output: Option<&Tref>,
    answer: &StubAnswer,
    refs: &mut Vec<Symbol>,
) -> String {
    match answer {
        StubAnswer::Value { value } => match output {
            Some(t) => format!(
                "  return {};\n",
                decoded_value_expr(t, value, refs, ctx.module)
            ),
            None => "  return;\n".to_string(),
        },
        StubAnswer::Error { error } => {
            let op = ctx.test.op.expect("an impl stub rides a call");
            let Some(err) = declared_tests::declared_error_by_shape(op, ctx.module, &error.shape)
            else {
                // Unreachable when the tests passed validation.
                return format!(
                    "  throw new Error({});\n",
                    ts_str(&format!("test names unknown error shape {}", error.shape))
                );
            };
            let ty = type_ident_from_id(&err.shape_id);
            refs.push(module_symbol(&format!("{ty}Error"), ctx.module));
            refs.push(module_symbol(&format!("decode{ty}"), ctx.module));
            format!(
                "  throw new {ty}Error(decode{ty}({data}), {body});\n",
                data = json_text(&error.data),
                body = ts_str(&json_text(&error.data)),
            )
        }
        StubAnswer::Contract { .. } => "  throw new Error(\"simulated bespoke failure\");\n".into(),
        // Rejected by validation: an http answer never reaches an impl stub.
        StubAnswer::Http(_) => "  throw new Error(\"unsupported stub answer\");\n".into(),
    }
}

/// The canned raw outcome: a value carries the wire body, an error carries the
/// code the glue discriminates on, a contract answer fails outright. The
/// literal is contextually typed by the swapper's `typeof` parameter, so the
/// ext runtime needs no import here.
fn raw_answer_body(ctx: &TestCtx<'_>, answer: &StubAnswer) -> String {
    match answer {
        StubAnswer::Value { value } => format!(
            "  return {{ success: true, code: \"\", body: {} }};\n",
            ts_str(&json_text(value)),
        ),
        StubAnswer::Error { error } => {
            let op = ctx.test.op.expect("an impl stub rides a call");
            let code = declared_tests::declared_error_by_shape(op, ctx.module, &error.shape)
                .and_then(|e| e.code)
                .map(|c| c.value)
                .unwrap_or_default();
            format!(
                "  return {{ success: false, code: {}, body: {} }};\n",
                ts_str(&code),
                ts_str(&json_text(&error.data)),
            )
        }
        StubAnswer::Contract { .. } => "  throw new Error(\"simulated bespoke failure\");\n".into(),
        // Rejected by validation: an http answer never reaches an impl stub.
        StubAnswer::Http(_) => "  return { success: false, code: \"\", body: \"\" };\n".into(),
    }
}

/// The invocation of the generated method plus the assertions the outcome
/// pattern dictates.
fn invoke_and_expect(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    let call = ctx.test.call.expect("invocation needs a call");
    let op = ctx.test.op.expect("a call resolved its op");
    let method = super::surface::method_name(op, ctx.config);
    let (input, output) = op_io(op);
    let mut text = String::new();
    let call_input = match (input, &call.input) {
        (Some(t), Some(value)) => {
            text.push_str(&format!(
                "const input = {};\n",
                decoded_value_expr(t, value, refs, ctx.module)
            ));
            "input"
        }
        _ => "",
    };
    let invocation = format!("c.{method}({call_input})");
    match ctx.test.outcome {
        None | Some(TestPattern::Ok(_)) => {
            text.push_str(&format!("await {invocation};\n"));
        }
        Some(TestPattern::Eq(value)) => {
            text.push_str(&format!("const out = await {invocation};\n"));
            let t = output.expect("validation ties eq to an output");
            text.push_str(&eq_assert(t, value, refs, ctx.module));
        }
        Some(TestPattern::Struct(pattern)) => {
            text.push_str(&format!("const out = await {invocation};\n"));
            let t = output.expect("validation ties a struct pattern to an output");
            text.push_str(&struct_asserts(t, pattern, refs, ctx.module));
        }
        Some(other) => {
            text.push_str(&format!(
                "let caught: unknown;\ntry {{\n  await {invocation};\n}} catch (e) {{\n  caught = e;\n}}\n"
            ));
            text.push_str(&failure_asserts(ctx, other, refs));
        }
    }
    if let Some(patterns) = ctx.test.requests {
        text.push_str(&request_asserts(patterns));
    }
    text
}

/// The canned answer of a free extern-fn stub, as the value the seam's swap
/// installs: a plain arrow returning the decoded value (the extern's
/// declared `returns:`-projected logical value, or the field's own declared
/// type -- both stub kinds only ever answer a single `Value`, matching
/// `validate_extern_stub`'s own restriction to `StubAnswer::Value` for a
/// free function).
fn extern_stub_answer_expr(
    ctx: &TestCtx<'_>,
    target: &Tref,
    stub: &crate::ir::ExternStub,
    refs: &mut Vec<Symbol>,
) -> String {
    let value = match stub.answers.first() {
        Some(StubAnswer::Value { value }) => value,
        // Rejected by `validate_extern_stub`: a free-fn stub answers a plain
        // value only.
        _ => &serde_json::Value::Null,
    };
    format!(
        "async () => {}",
        decoded_value_expr(target, value, refs, ctx.module)
    )
}

/// Every free extern-fn stub this test declares, wrapped as nested
/// swap/restore pairs around `body`: `entries::plan` and
/// `declared_tests::reachable_externs` guarantee every extern call the
/// construction path reaches is covered by exactly one of these before
/// generation reaches here, so this only has to spell the swap itself, not
/// validate coverage again. A handle-method stub (`ExternStubTarget::Method`)
/// has nothing to wrap yet: no target's codegen consumes an op's own
/// `impl_call` body (see `ir.rs`'s note by `OpImplCall`), so a test that
/// reaches one is unreachable through this target today.
fn wrap_extern_stubs(ctx: &TestCtx<'_>, body: String, refs: &mut Vec<Symbol>) -> String {
    let mut wrapped = body;
    for stub in &ctx.test.extern_stubs {
        let crate::ir::ExternStubTarget::Free { lib, fn_ } = &stub.target else {
            panic!(
                "a handle-method extern stub was planned for a TypeScript test, but no TypeScript \
                 codegen consumes an op's own impl_call body yet"
            );
        };
        let field = ctx
            .entry
            .fields
            .iter()
            .find(|f| {
                f.call
                    .as_ref()
                    .is_some_and(|c| &c.ns == lib && &c.func == fn_)
            })
            .unwrap_or_else(|| {
                panic!(
                    "extern stub on '{lib}.{fn_}' names no field of entry '{}'",
                    ctx.entry.name
                )
            });
        let swap = super::ext_call::ext_swap_fn_name(ctx.n, field);
        refs.push(module_symbol(&swap, ctx.module));
        let answer = extern_stub_answer_expr(ctx, &field.target, stub, refs);
        wrapped = format!(
            "const prev{Field} = {swap}({answer});\n\
             try {{\n{inner}}} finally {{\n  {swap}(prev{Field});\n}}\n",
            Field = super::pascal(&field.name),
            inner = indent(&wrapped, "  "),
        );
    }
    wrapped
}

/// One hermetic test: pin the environment, install the declared stub, build
/// the client through the real construction path, run the call, assert. A
/// construction-only test just constructs and asserts its outcome.
fn hermetic_case(ctx: &TestCtx<'_>, uses_env: &mut bool, refs: &mut Vec<Symbol>) -> String {
    refs.push(module_symbol(&ctx.n.client, ctx.module));
    let args = construction_args(ctx);
    let body = if ctx.test.call.is_some() {
        let stub = ctx.test.stub.expect("a hermetic call has its stub");
        match stub.dep {
            StubDep::Http => {
                refs.push(support_symbol("HttpRequest"));
                refs.push(support_symbol("HttpTransport"));
                let answers: Vec<&HttpAnswer> = stub
                    .answers
                    .iter()
                    .filter_map(|a| match a {
                        StubAnswer::Http(h) => Some(h),
                        _ => None,
                    })
                    .collect();
                let seam_call = if args.is_empty() {
                    "{ transport }".to_string()
                } else {
                    format!("{{ transport }}, {args}")
                };
                format!(
                    "{setup}const c = {client}.forTest({seam_call});\n{invoke}",
                    setup = transport_block(&answers),
                    client = ctx.n.client,
                    invoke = invoke_and_expect(ctx, refs),
                )
            }
            StubDep::Impl => {
                let op = ctx.test.op.expect("an impl stub rides a call");
                let swap = super::impl_op::swap_fn_name(ctx.n, op);
                refs.push(module_symbol(&swap, ctx.module));
                let bodies: Vec<String> = stub
                    .answers
                    .iter()
                    .map(|answer| answer_body(ctx, answer, refs))
                    .collect();
                let stub_body = if bodies.len() == 1 {
                    bodies.into_iter().next().unwrap_or_default()
                } else {
                    let mut arms = String::new();
                    for (i, b) in bodies.iter().enumerate() {
                        if i + 1 == bodies.len() {
                            arms.push_str(&format!(
                                "    default: {{\n{}    }}\n",
                                indent(b, "    ")
                            ));
                        } else {
                            arms.push_str(&format!(
                                "    case {i}: {{\n{}    }}\n",
                                indent(b, "    ")
                            ));
                        }
                    }
                    format!(
                        "  const i = Math.min(implCalls, {last});\n\
                         \x20 implCalls += 1;\n\
                         \x20 switch (i) {{\n{arms}  }}\n",
                        last = bodies.len() - 1,
                    )
                };
                let counter = if stub.answers.len() > 1 {
                    "let implCalls = 0;\n"
                } else {
                    ""
                };
                let inner = format!(
                    "const c = new {client}({args});\n{invoke}",
                    client = ctx.n.client,
                    invoke = invoke_and_expect(ctx, refs),
                );
                // The seam is module state, so the swap is restored however
                // the case ends (these cases do not run concurrently within
                // the file).
                format!(
                    "{counter}const prevImpl = {swap}(async () => {{\n{stub_body}}});\n\
                     try {{\n{inner}}} finally {{\n  {swap}(prevImpl);\n}}\n",
                    inner = indent(&inner, "  "),
                )
            }
        }
    } else {
        // Construction-only: the outcome pattern reads the construction error.
        match ctx.test.outcome {
            None | Some(TestPattern::Ok(_)) => format!(
                "new {client}({args});\n",
                client = ctx.n.client,
            ),
            Some(pattern) => format!(
                "let caught: unknown;\ntry {{\n  new {client}({args});\n}} catch (e) {{\n  caught = e;\n}}\n{asserts}",
                client = ctx.n.client,
                asserts = failure_asserts(ctx, pattern, refs),
            ),
        }
    };
    let body = wrap_extern_stubs(ctx, body, refs);
    let pins = env_pins(ctx);
    let inner = if pins.is_empty() {
        indent(&body, "  ")
    } else {
        *uses_env = true;
        // The previous values of every touched variable are saved up front and
        // restored however the case ends, so resolution stays deterministic on
        // any machine; an undefined restores by deleting (a set empty string
        // would still count as present).
        let prev: Vec<String> = pins
            .iter()
            .map(|(name, _)| format!("{key}: vectorEnv[{key}]", key = ts_str(name)))
            .collect();
        let mut setup = format!(
            "const prevEnv: Record<string, string | undefined> = {{ {} }};\n",
            prev.join(", "),
        );
        for (name, value) in &pins {
            match value {
                Some(v) => {
                    setup.push_str(&format!("vectorEnv[{}] = {};\n", ts_str(name), ts_str(v)))
                }
                None => setup.push_str(&format!("delete vectorEnv[{}];\n", ts_str(name))),
            }
        }
        let restore = "for (const [name, value] of Object.entries(prevEnv)) {\n\
                       \x20 if (value === undefined) {\n\
                       \x20   delete vectorEnv[name];\n\
                       \x20 } else {\n\
                       \x20   vectorEnv[name] = value;\n\
                       \x20 }\n\
                       }\n";
        let guarded = format!(
            "{setup}try {{\n{body}}} finally {{\n{restore}}}\n",
            body = indent(&body, "  "),
            restore = indent(restore, "  "),
        );
        indent(&guarded, "  ")
    };
    format!(
        "it({name}, async () => {{\n{inner}}});\n",
        name = ts_str(ctx.test.name),
    )
}

/// One live test: no stub and no pinned environment; construction reads the
/// ambient env (real credentials), and the same expectations verify that the
/// spec still matches the real dependency.
fn live_case(ctx: &TestCtx<'_>, refs: &mut Vec<Symbol>) -> String {
    refs.push(module_symbol(&ctx.n.client, ctx.module));
    let body = format!(
        "const c = new {client}({args});\n{invoke}",
        client = ctx.n.client,
        args = construction_args(ctx),
        invoke = invoke_and_expect(ctx, refs),
    );
    format!(
        "it({name}, async () => {{\n{body}}});\n",
        name = ts_str(ctx.test.name),
        body = indent(&body, "  "),
    )
}

/// The one per-file declaration a test file may carry: the ambient process
/// env as a value binding, which the env pins mutate and the live gate reads.
fn env_const_decl() -> Decl {
    Decl::raw(
        "// The ambient process env, reached through globalThis so the generated\n\
         // test does not require node type declarations.\n\
         const vectorEnv: Record<string, string | undefined> =\n\
         \x20 (globalThis as { process?: { env?: Record<string, string | undefined> } })\n\
         \x20   .process?.env ?? {};"
            .to_string(),
    )
}

#[cfg(test)]
#[path = "vector_tests_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "vector_expects_tests.rs"]
mod expects_tests;

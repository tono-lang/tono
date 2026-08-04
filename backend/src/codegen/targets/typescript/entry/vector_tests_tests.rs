//! The generated-Vitest tests, split from the emitter so each file stays
//! within the repo size gate. The declared tests come from the shared bed;
//! only the assertions over the generated TypeScript live here.

use super::super::tests::fixture_module;
use crate::codegen::targets::typescript::types::ts_casing;
use crate::codegen::targets::typescript::TsRules;
use crate::codegen::test_support::{impl_extension, notes_bed, rendered, wired, with_tests};
use crate::ir::TestPattern;

/// An `ext impl` binding for the fixture's `save_note`.
fn impl_ext() -> crate::ir::Extension {
    impl_extension("ts", "save_note", "ext/ts/save.ts#saveNote", false)
}

fn full_text(module: &crate::ir::Module) -> String {
    let emission = super::super::emit(module, &ts_casing());
    let mut decls = emission.shared;
    decls.extend(emission.per_entry.into_iter().flat_map(|(_, d)| d));
    rendered(&decls, &TsRules)
}

#[test]
fn declared_tests_swap_the_construction_and_impl_seams() {
    let mut module = with_tests(fixture_module(), notes_bed().impl_echo_tests());
    module.extensions = vec![impl_ext()];
    let out = full_text(&module);
    // The method reaches the bespoke symbol through the module-local seam
    // (an ESM import is read-only), and the swapper is exported only
    // because the entry declares tests.
    assert!(out.contains("let saveNoteImpl = saveNote;"));
    assert!(out.contains("return await saveNoteImpl(this.settings, input);"));
    assert!(out.contains(
        "export function swapSaveNoteImplForTest(next: typeof saveNote): typeof saveNote {"
    ));
    // The class gains the transport seam the generated tests construct
    // through; the override lands after construction ran, so the test
    // transport wins over anything bespoke.
    assert!(out.contains(
        "static forTest(seam: { transport: CanonicalTransport }, apiKey: string, config: ClientConfig = {}): Client {"
    ));
    assert!(out.contains("options.transport = seam.transport;"));
    assert!(out.contains("options.fetch = undefined;"));
    // Without declared tests the impl seam stays, the test surface does not.
    module.tests.clear();
    let plain = full_text(&module);
    assert!(plain.contains("let saveNoteImpl = saveNote;"));
    assert!(!plain.contains("swapSaveNoteImplForTest"));
    assert!(!plain.contains("forTest"));
}

#[test]
fn declared_tests_generate_a_hermetic_and_a_live_vitest_file() {
    let mut module = with_tests(fixture_module(), notes_bed().impl_echo_tests());
    module.extensions = vec![impl_ext()];
    let files = super::test_files(&module, &ts_casing());
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].group.tests_of(), Some(("client", false)));
    let hermetic = rendered(&files[0].file.decls, &TsRules);
    // The test swaps the impl seam, restores it in finally, and runs the
    // real method through the real construction path.
    assert!(hermetic.contains("describe(\"client\", () => {"));
    assert!(hermetic.contains("it(\"stores it\", async () => {"));
    assert!(hermetic.contains("const prevImpl = swapSaveNoteImplForTest(async () => {"));
    assert!(hermetic.contains("swapSaveNoteImplForTest(prevImpl);"));
    assert!(hermetic.contains("const c = new Client(\"k\");"));
    assert!(hermetic.contains("await c.saveNote(input);"));
    // The @arg value comes from the pinned construction values; env chains
    // not covered by them are pinned absent through an inline try/finally
    // (save the touched variables, delete for absence, restore with the
    // undefined-deletes dance) so resolution is deterministic anywhere.
    assert!(hermetic.contains("const prevEnv: Record<string, string | undefined> = {"));
    assert!(hermetic.contains(": vectorEnv["));
    assert!(hermetic.contains("delete vectorEnv["));
    assert!(hermetic.contains("} finally {"));
    assert!(hermetic.contains("for (const [name, value] of Object.entries(prevEnv)) {"));
    // The wire spelling is what is compared, never the language spelling.
    assert!(hermetic.contains("expect(encodeNote(out)).toEqual({\"id\":\"n1\"});"));
    // The only file-level declaration is the env value binding: no helper
    // functions ride the generated file.
    assert!(!hermetic.contains("function "));
    // The live test constructs off the ambient environment, gated behind
    // the opt-in variable so it stays out of a default run.
    assert_eq!(files[1].group.tests_of(), Some(("client", true)));
    let live = rendered(&files[1].file.decls, &TsRules);
    assert!(live.contains(
        "describe.runIf(vectorEnv[\"TONO_LIVE_TESTS\"] === \"1\")(\"client (live)\", () => {"
    ));
    assert!(live.contains("it(\"hits the real store\", async () => {"));
    assert!(!live.contains("prevEnv"));
    assert!(!live.contains("function "));
}

#[test]
fn an_http_stub_generates_a_request_matching_test() {
    let bed = notes_bed();
    let module = wired(
        fixture_module(),
        vec![bed.retry_request_test("/notes", TestPattern::Eq(bed.input.clone()))],
    );
    let files = super::test_files(&module, &ts_casing());
    assert_eq!(files.len(), 1);
    let text = rendered(&files[0].file.decls, &TsRules);
    // The stubbed transport records every canonical request and answers the
    // canned sequence, the last response repeating; construction goes through
    // the class seam.
    assert!(text.contains("seen.push(req);"));
    assert!(text.contains("{ status: 500, headers: {}, body: \"{\\\"id\\\":\\\"n1\\\"}\" },"));
    assert!(text.contains("return responses[Math.min(seen.length - 1, responses.length - 1)];"));
    assert!(text.contains("const c = Client.forTest({ transport }, \"k\");"));
    // The whole request-pattern list matches all recorded requests with equal
    // length, in order; the headers compare through one lowercased copy per
    // request, with no helper function and no superfluous cast.
    assert!(text.contains("expect(seen.length).toBe(2);"));
    assert!(text.contains("const req0 = seen[0];"));
    assert!(text.contains("const req1 = seen[1];"));
    assert!(text.contains("expect(req0.method).toBe(\"POST\");"));
    assert!(text.contains("expect(new URL(req1.url).pathname).toBe(\"/notes\");"));
    assert!(text.contains("const lower0 = Object.fromEntries("));
    assert!(text
        .contains("Object.entries(req0.headers).map(([k, v]) => [k.toLowerCase(), v] as const)"));
    assert!(text.contains("expect(lower0[\"authorization\"]).toBe(\"Bearer k\");"));
    assert!(!text.contains("as CanonicalRequest"));
    assert!(!text.contains("function "));
}

#[test]
fn a_single_http_answer_returns_directly_without_sequence_machinery() {
    let bed = notes_bed();
    let module = wired(
        fixture_module(),
        vec![bed.outcome_test("answers once", TestPattern::Eq(bed.input.clone()))],
    );
    let files = super::test_files(&module, &ts_casing());
    assert_eq!(files.len(), 1);
    let text = rendered(&files[0].file.decls, &TsRules);
    assert!(
        text.contains("return { status: 200, headers: {}, body: \"{\\\"id\\\":\\\"n1\\\"}\" };")
    );
    assert!(!text.contains("const responses"));
    assert!(!text.contains("Math.min"));
}

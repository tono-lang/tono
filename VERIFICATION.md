# Verification

How we trust the generated SDKs. Code generation *relocates* verification rather
than removing it: the gates below sit where the risk is, and are proportional to
it. Property and round-trip are light on the mechanical parts; the mutation gate
is strong on the hand-written (bespoke) seam.

## Layers and gates

| Layer | Technique | Gate | Where |
|---|---|---|---|
| Types / serialization | property (round-trip + idempotence) | light | `frontend/test/test_property.ml` (QCheck), `backend/tests/property_ir.rs` (proptest) |
| Calculus | golden | light | `frontend/test/calculus_test.ml` |
| Bespoke (codecs + error taxonomy) | golden + differential + **mutation** | **strong** | `backend/tests/conformance.rs`, `.cargo/mutants.toml` |
| User bespoke (ext impls, live API) | conformance vectors -> generated native tests | opt-in per operation | `examples/*/vectors/*.json`, emitted `*_test.go` / `*.test.ts` / `*_test.rs` |
| HTTP transport (entry client) | codegen snapshot | review | `backend/src/codegen/targets/*/entry/transport*.rs`, `backend/tests/snapshot_codegen.rs` |
| Runtime parity (retry/timeout/errors) | shared behavior vectors | breaks build | `runtimes/parity/vectors.json`; TypeScript, Go, and Rust all drive a generated SDK compiled from `runtimes/parity/spec.tono` (`scripts/run-parity.sh`) |
| Generator (codegen) | snapshot | review | `backend/tests/snapshot_codegen.rs` |
| FFI (`ext`/`extern`) against real library shapes | bench: compile and run the generated SDK against stand-in libraries, per capability, with a recorded expected outcome | breaks build on drift from the record (regression or progress) | `examples/mathkit/` (`gate.tsv`, `README.md`), `scripts/check-ffi-bench.sh` |
| IR (frontend <-> backend) | round-trip | breaks build | `backend/tests/ir_roundtrip.rs`, `frontend/test/golden_test.ml` |

Two of these are not behavior checks and sit outside the bespoke-vs-rest axis:
the IR round-trip is a **contract sanity** gate (a frontend/backend divergence
breaks the build unconditionally), and the codegen snapshot is a **review** gate
(an unexpected diff needs a human to accept it, it does not block on its own).

## Property tests

- **Round-trip**: `decode(encode(m)) == m` as JSON data, over randomized models.
  The oracle is the relation itself, so no reference implementation is needed.
- **Idempotence**: re-encoding a decoded document is a fixed point.

Run: `cargo test --test property_ir` (Rust) and `dune runtest` (OCaml).

## Differential

The same IR through the TypeScript, Rust, and Go targets must produce canonically
equal wire JSON. This collapses "which of N is right?" into "do all N agree?" and
catches drift between ports.

Run: `cargo test --test conformance` (needs the Go/Node toolchains).

## Generated native tests (user bespoke)

What only the user can get wrong (their `ext impl` code, their real API) has
its oracle outside the generator, so it is not covered by round-trips.
A conformance vector declares "with this input, with this point mocked, expect
this outcome"; the generator turns it into native test files beside the client
(`go test`, Vitest, `cargo test`), wiring the declared mock through seams the
SDK only grows when vectors exist. A case without a mock runs against the real
dependency and sits behind an opt-in marker (`//go:build live`, an env-gated
Vitest suite, `#[ignore]`), which is what catches drift between the vectors and
the real API. Deliberately absent: mechanical round-trip tests derived from the
IR, whose assertions would come from the same compiler as the code under test.

Run: `scripts/check-impl-conformance.sh` (also runs the generated tests).

## Codegen snapshot (review gate)

The emitted source for the shared wire-matrix module is snapshotted per target
with `insta`. An unexpected diff fails the test; an intended change is accepted by
a human:

```
cargo test --test snapshot_codegen   # fails on an unreviewed diff
cargo insta review                   # accept or reject each pending change
```

The snapshots are hermetic (rendered through the identity formatter), so the gate
needs no language formatter installed.

## Mutation (strong bespoke gate)

`cargo-mutants` mutates the hand-written seam (`ops.rs`, `targets/*/codecs.rs`,
`targets/*/errors.rs`; scope in `.cargo/mutants.toml`) and requires every mutant
to be caught. A surviving mutant means behavior no test pins down, and it fails
the build. Mechanical lowering is out of scope: it is covered by the snapshot
gate, not mutation.

```
cargo mutants            # exits non-zero if any bespoke mutant survives
cargo mutants --list     # the mutants in scope
```

A genuinely-equivalent mutant (one that cannot change observable behavior) is
annotated at the site with a one-line reason: `#[mutants::skip]`. There are no
blanket exclusions.

Each target's entry client now emits its own HTTP transport inline (retry,
timeout, error classification) instead of depending on a hand-written runtime
package; the templates that render it are not currently in the mutation gate's
scope (`.cargo/mutants.toml` covers `ops.rs`/`taxonomy.rs`/codecs/errors, not
`targets/*/entry/transport*.rs`). Correctness for that seam instead comes from
one shared behavior-vector suite (`runtimes/parity/vectors.json`): every target
runs the same retry, timeout, and error-classification scenarios with pinned
jitter and recorded backoff, so the targets cannot drift apart. Every target's
harness compiles `runtimes/parity/spec.tono`, generates the real SDK, and
drives that generated client directly (via `scripts/run-parity.sh`): TypeScript
through `runtimes/parity/typescript/parity.test.ts`, Go through
`runtimes/parity/go/parity_test.go`, and Rust through
`runtimes/parity/rust/parity_harness.rs`. The suite proves what a consumer
actually imports (the Rust harness runs as a `#[cfg(test)]` module of the
generated crate, which is what lets it pin the client's crate-visible retry
timing seam). Extending the mutation gate to the transport codegen templates
is a follow-up, not covered here.

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
| Bespoke runtime (HTTP transport) | property + **mutation** | **strong** | `runtimes/http-ts/test`, `runtimes/http-ts/stryker.config.json`, `runtimes/http-go/*_test.go`, `runtimes/http-go/.gremlins.yaml` |
| Runtime parity (retry/timeout/errors) | shared behavior vectors | breaks build | `runtimes/parity/vectors.json`, run by every HTTP runtime's test suite |
| Generator (codegen) | snapshot | review | `backend/tests/snapshot_codegen.rs` |
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

The hand-written HTTP transport runtimes are bespoke too, so they carry the
same strong gate through the mutation tool of their language:

```
cd runtimes/http-ts && npm run mutation   # StrykerJS, break threshold is 100
cd runtimes/http-go && gremlins unleash   # gremlins, thresholds in .gremlins.yaml
```

A genuinely-equivalent mutant (one that cannot change observable behavior) is
annotated at the site with a one-line reason: `#[mutants::skip]` on the Rust side,
`// Stryker disable next-line` in the TypeScript runtime. There are no blanket
exclusions. gremlins has no per-site annotation, so the Go runtime is written
to avoid equivalent mutants (no redundant guards) instead.

The HTTP runtimes also share one behavior-vector suite
(`runtimes/parity/vectors.json`): every runtime runs the same retry, timeout,
and error-classification scenarios with pinned jitter and recorded backoff, so
the runtimes cannot drift apart.

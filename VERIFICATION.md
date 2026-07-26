# Verification

How we trust the generated SDKs. Code generation *relocates* verification rather
than removing it: the gates below sit where the risk is, and are proportional to
it. Property and round-trip are light on the mechanical parts; the mutation gate
is strong on the hand-written (bespoke) seam.

## Layers and gates

| Layer | Technique | Gate | Where |
|---|---|---|---|
| Types / serialization | property (round-trip + idempotence) | light | `frontend/test/test_property.ml` (QCheck), `backend/tests/property_ir.rs` (proptest) |
| Calculus | golden (self-referential vectors) | light | `frontend/test/calculus_vectors.json`, `frontend/test/calculus_vectors_test.ml`, `frontend/test/calculus_test.ml` |
| Bespoke (codecs + error taxonomy) | golden (N vs 1) + differential + **mutation** | **strong** | `backend/tests/golden_wire.rs` + `backend/tests/golden/wire_vectors.json`, `backend/tests/conformance.rs`, `.cargo/mutants.toml` |
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

## Calculus (self-referential golden)

The calculus needs no external reference: the frontend evaluator *is* the truth,
and a well-typed program compiles into equivalent total code by construction.
The committed vectors in `frontend/test/calculus_vectors.json` pin the
evaluator's input-to-output relation (wrapping, truncated division, coercions,
collection builtins, match with the `Unknown` arm); a target that lowers the
calculus must reproduce the same outputs from the same file. Because only the
lowering needs pinning, the gate is light: golden vectors, no mutation.

Run: `dune runtest` (the `calculus_vectors` suite).

## Golden (N versus 1, designated reference)

The bespoke wire behavior elects one designated reference port (Rust). The
vectors in `backend/tests/golden/wire_vectors.json` carry, for each wire input,
the output the reference produced; every generated port re-encodes the same
inputs and must match. This collapses "which of N is right?" into "all agree
with the reference". The reference is also checked against its own committed
vectors, so reference drift is visible too; an intended change is regenerated
deliberately and reviewed as a diff:

```
cargo test --test golden_wire                          # every port vs the vectors
TONO_UPDATE_GOLDEN_WIRE=1 cargo test --test golden_wire  # regenerate from the reference
```

## Differential

The same batch of wire documents (the canonical fixture plus seeded
pseudo-random documents) runs through the TypeScript, Rust, and Go ports, and
the re-encoded JSON must agree pairwise. Unlike the golden harness this does not
say which port is right; it catches drift between ports on inputs nobody
hand-picked. The batch is deterministic (fixed seed) so CI is reproducible;
`TONO_DIFFERENTIAL_SEED` explores other batches locally.

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

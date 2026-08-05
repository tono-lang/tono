# Cross-runtime parity

Proves that retry, timeout, and error-classification behavior does not drift
across the three HTTP runtimes. The pieces:

- **`spec.tono`**: a small `.tono` module declaring the operation shapes the
  vectors exercise (with and without `@retry`, with `@timeout`, errors
  sharing a status discriminated by `@errorCode`, a labeled path with
  declared headers). Compiled fresh by the TypeScript harness; not used by
  Go or Rust yet.
- **`vectors.json`**: the shared behavior vectors. Each names an operation
  in `spec.tono` (`op`), the client construction values that vary per vector
  (`config`: `max_retries`, `timeout_ms`), a scripted sequence of transport
  outcomes (`script`), and the expected result (`expect`: outcome, attempts,
  backoff delays). See the file's own top-level `comment` for the exact
  contract, including why four vectors that used to live here are gone.
- **`typescript/parity.test.ts`**: the TypeScript harness. Unlike the other
  two, it does not talk to a hand-written runtime package: it is copied
  next to a freshly generated SDK (compiled from `spec.tono`) and drives
  that SDK's real generated client, pinning the generated `timingSeam`
  (in the SDK's own `http.ts`) so retry backoff is deterministic and instant.
- **`../../scripts/run-parity.sh`**: builds the frontend and backend CLIs,
  compiles `spec.tono`, generates the SDK into a throwaway directory, drops
  the harness in next to it, and runs the suite with Vitest (reusing
  `runtimes/http-ts`'s own toolchain rather than provisioning a second one).
  Currently only knows the `typescript` target.

Go (`runtimes/http-go/parity_test.go`) and Rust (`runtimes/http-rust/src/parity.rs`)
are not repointed yet: they still build a synthetic `WireDescriptor` and call
their runtime package's `Execute`/`execute` directly, the way TypeScript used
to. They read the same `vectors.json`, reconstructing a descriptor from each
vector's `op` + `config` through a small local lookup table (`descriptorFor`
in Go, `descriptor_for` in Rust) in place of the retired `descriptor` field.
Repointing them to their own generated SDKs is separate follow-up work; when
it happens, `spec.tono` and `vectors.json` are already shaped for it.

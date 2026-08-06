# Payments example

A worked end-to-end example: a two-module `.tono` project compiled all the way to
SDKs in every target language.

```
src/**.tono ──frontend──▶ ir.json ──tono gen──▶ sdk/{rust,go,typescript}
```

- [`src/payments/`](src/payments) — the source, split into two modules so it
  exercises the module system:
  - [`common.tono`](src/payments/common.tono) is `payments.common`. It exports a
    `pub` enum and a `pub` union; its `card` and `bank_account` structs are
    private, visible only inside the module where the union folds them in.
  - [`charges.tono`](src/payments/charges.tono) is `payments.charges`. It
    `import`s `payments.common` and references its exported types qualified
    (`common.status`, `common.payment_method`). The `charge` struct is chosen to
    exercise the hard wire cases: 64-bit integers (string on the wire), `bytes`
    (base64), an open enum and an int-backed enum, an internally-tagged union, a
    nullable field, a list, a map, and the well-known `uuid`/`timestamp` types.
    It also declares a `client` entry: a struct with the operation in its body,
    whose fields carry construction sources (`@arg` for the API key, `@env` with
    a `@default` for the endpoint, `@with` for timeout and retries). The entry
    becomes the SDK construction surface: `New(apiKey, ...)` with functional
    options in Go, `new Client(apiKey, config?)` in TypeScript, each resolving
    the declared sources, validating, and freezing the resolved values for the
    runtime. The `@http` binding is resolved into a typed wire binding that
    each target's entry codegen renders into its own inline HTTP transport;
    the binding's endpoint, header, timeout, and retry references resolve
    against those frozen values.
- [`ir.json`](ir.json) — the canonical IR the frontend emits (the contract the
  backend consumes). Shape ids are module-qualified (`payments.charges#charge`).
- [`sdk/`](sdk) — the generated source, laid out in emission groups. Each module
  is a directory holding one file per group: `types` (the public surface),
  `codec` (the serialization of those types), one named after each entry
  declaration (`client`), and `internal` (whatever no public type reaches).
  Everything a consumer must not reach is fenced off by the target's own
  mechanism: in Go a package under `internal/`, which the toolchain refuses to
  resolve from outside the SDK, so the module's own package holds only files
  named for what they contain; in Rust a crate-visible `mod`; in TypeScript a
  file left out of the package's `exports`. What crosses module boundaries sits
  in an SDK-root group: `support` for what a consumer names (the branded
  well-known types, so two modules' `Timestamp` are one type), `internal` for
  the runtime helpers. Each module re-exports
  its public groups (a `pub use` in Rust, a barrel in TypeScript); there is no
  root barrel aggregating the modules.

## Generated — do not edit

Everything under `ir.json` and `sdk/` is generated; edit the sources under `src/`
(or the compiler) and regenerate:

```sh
scripts/regen-example.sh
```

CI runs the same script and fails if the result differs from what is committed,
so this example always matches what the current compiler produces, and a second
check compiles all three generated SDKs to prove they are correct.

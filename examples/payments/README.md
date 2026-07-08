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
    It also declares an async HTTP operation with two declared errors, so the
    SDKs carry the error taxonomy, the client surface, and per-operation error
    discrimination. The `@http` binding is resolved into an opaque wire descriptor
    the TypeScript SDK embeds and hands to the hand-written runtime
    ([`runtimes/http-ts`](../../runtimes/http-ts)) at `runtime.execute`.
- [`ir.json`](ir.json) — the canonical IR the frontend emits (the contract the
  backend consumes). Shape ids are module-qualified (`payments.charges#charge`).
- [`sdk/`](sdk) — the generated source. Each module maps to an idiomatic
  sub-package: Rust modules under `rust/payments/` (with a generated `mod.rs`
  tree), Go packages under `go/payments/`, and TypeScript sub-paths under
  `typescript/payments/`.

## Generated — do not edit

Everything under `ir.json` and `sdk/` is generated; edit the sources under `src/`
(or the compiler) and regenerate:

```sh
scripts/regen-example.sh
```

CI runs the same script and fails if the result differs from what is committed,
so this example always matches what the current compiler produces, and a second
check compiles all three generated SDKs to prove they are correct.

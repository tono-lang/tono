# Payments example

A worked end-to-end example: one `.tono` source compiled all the way to SDKs in
every target language.

```
payments.tono ──frontend──▶ ir.json ──tono gen──▶ sdk/{rust,go,typescript}
```

- [`payments.tono`](payments.tono) — the source. A small payments API chosen to
  exercise the hard wire cases: 64-bit integers (string on the wire), `bytes`
  (base64), an open enum and an int-backed enum, an internally-tagged union, a
  nullable field, a list, a map, and the well-known `uuid`/`timestamp` types.
- [`ir.json`](ir.json) — the canonical IR the frontend emits (the contract the
  backend consumes).
- [`sdk/`](sdk) — the generated source, one file per language.

## Generated — do not edit

Everything under `ir.json` and `sdk/` is generated; edit `payments.tono` (or the
compiler) and regenerate:

```sh
scripts/regen-example.sh
```

CI runs the same script and fails if the result differs from what is committed,
so this example always matches what the current compiler produces.

# Bearer auth recipe

Auth in tono is 100% bespoke: there is no built-in scheme. This recipe shows the
smallest thing that works, a `before_request` hook that sets the `Authorization`
header on every request.

```
auth.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
   │
   └── ext/ts/auth.ts   (the bespoke hook the SDK binds to)
```

This recipe is source only (the `.tono` plus the bespoke hook); regenerate the
SDK yourself with the commands below.

## The pieces

- [`auth.tono`](auth.tono) declares a tiny API and one extension:

  ```
  ext hook before_request {
    ts: "ext/ts/auth.ts#addBearer"
  }
  ```

  `before_request` is one of the four closed lifecycle slots
  (`client_init`, `before_request`, `after_response`, `on_error`). The binding
  points at a per-language source file (`file#symbol`).

- [`ext/ts/auth.ts`](ext/ts/auth.ts) is the hand-written hook. The runtime hands
  it the `CanonicalRequest` before sending; it returns a copy with the header
  set. The generated SDK wraps the call at the boundary, so a throw here surfaces
  as a `ContractError`.

- The generated SDK's `HttpClient` imports `addBearer` and hands it to the
  runtime as a hook, so every call it makes carries the header with no per-call
  plumbing.

## Adapting it

Swap the token source in `ext/ts/auth.ts` for your own (an environment variable,
a config value, a secret store). A network refresh needs the async hook slot,
which is not yet available. The same shape ports to the other targets by binding
`rust`/`go`/`python`/`java` to their own source files.

## Regenerating

```
frontend compile auth.tono --module auth > ir.json
tono gen --target typescript --out sdk < ir.json
```

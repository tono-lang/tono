# Bearer auth recipe

Auth in tono is 100% bespoke: there is no built-in scheme. This recipe shows the
smallest thing that works: the entry declares where the token comes from
(`@env("API_TOKEN")`), and a `client_init` hook turns the resolved value into an
`Authorization` header. The hook never reads the environment itself; the
declared sources already did.

```
auth.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
   │
   ├── ext/go/auth.go            (the bespoke hook, in Go)
   └── ext/ts/auth.ts            (the bespoke hook, in TypeScript)
```

This recipe is source only (the `.tono` plus the bespoke hook); regenerate the
SDK yourself with the commands below.

## The pieces

- [`auth.tono`](auth.tono) declares a `client` entry (a struct with the
  operation in its body) and one extension:

  ```
  ext hook client_init {
    go: "ext/go/auth.go#ApplyBearer"
    ts: "ext/ts/auth.ts#applyBearer"
  }
  ```

  `client_init` is one of the four closed lifecycle slots (`client_init`,
  `before_request`, `after_response`, `on_error`). The binding points at a
  per-language source file (`file#symbol`).

- [`ext/ts/auth.ts`](ext/ts/auth.ts) is the hand-written hook. The generated
  constructor resolves the entry's declared sources into a `Settings` value
  (the resolved fields plus the transport slots), then hands it to this hook:
  bespoke code runs on top (bespoke wins) and writes the header into
  `settings.headers`, which the runtime applies to every request. Validation
  runs after the hook. A throw here surfaces as a `ContractError`.

- The generated SDK exports a `Client` class per entry; `new Client()` performs
  the whole flow (sources, bridge, validation) so every call carries the header
  with no per-call plumbing.

## The generated tests

Bespoke request work is exactly what generated round-trip checks cannot verify;
the oracle has to sit outside the generator. The `test` blocks in `auth.tono`
declare it: with the transport stubbed, calling `get_account` must put
`authorization: Bearer <token>` on the wire, and constructing with an empty
token must fail as a `client_init` contract error. The generator emits them as
native tests beside the client; if the hook stops writing the header, the test
fails.

## Adapting it

Swap the header logic in `ext/ts/auth.ts` for your own scheme (a different
header, a signature over the request). Per-request work (signing bodies,
refreshing tokens) belongs in a `before_request` hook, which sees the declared
headers already applied. The same shape ports to the other targets by binding
`go`/`rust` to their own source files.

## Regenerating

```
frontend compile auth.tono --module auth > ir.json
tono gen --target typescript --out sdk < ir.json
cp -R ext sdk/typescript/   # the serde file imports the hook relative to itself
```

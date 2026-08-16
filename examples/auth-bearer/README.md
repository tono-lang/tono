# Bearer auth recipe

Auth in tono is 100% bespoke: there is no built-in scheme. This recipe shows
the smallest thing that works, and it needs no bespoke code at all: the entry
declares where the token comes from (`@env("API_TOKEN")`), a declared field
derives the header from it (`@format("Bearer {.api_token}")`), and a trait
attaches that header to every request (`@header("Authorization", .auth_header)`).

```
auth.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
```

This recipe is source only; regenerate the SDK yourself with the commands
below.

## The pieces

- [`auth.tono`](auth.tono) declares a `client` entry (a struct with the
  operation in its body) and three fields that chain into the header:

  ```
  api_token: string @env("API_TOKEN") @length(min: 1)
  auth_header: string @format("Bearer {.api_token}")
  ...
  op get_account(): account
    @http(method: "GET", path: "/account", endpoint: .endpoint)
    @header("Authorization", .auth_header)
  ```

  `api_token` resolves from the environment and is validated non-empty;
  `auth_header` is a pure declared transform over it; `@header` reads
  `auth_header` when the request is assembled. Every step is visible by
  reading the entry, nothing runs at a hidden lifecycle point.

- The generated SDK exports a `Client` class per entry; `new Client()`
  performs the whole flow (sources, validation) so every call carries the
  header with no per-call plumbing.

## The generated tests

The `test` blocks in `auth.tono` declare the two properties that matter: with
the transport stubbed, calling `get_account` must put
`authorization: Bearer <token>` on the wire, and constructing with an empty
token must fail as a `ValidationError` naming `api_token`. The generator
emits them as native tests beside the client.

## Adapting it

Swap `auth_header`'s `@format` for your own scheme (a different header
shape), or reach for the `ext <lib> { extern ... }` FFI block when the
scheme needs an external library instead of a pure derivation (signing a
request, calling a token-exchange SDK). See `examples/` for a library
integration recipe.

## Regenerating

```
frontend compile auth.tono --module auth > ir.json
tono gen --target typescript --out sdk < ir.json
```

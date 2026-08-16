# External library integration recipe

Some construction values genuinely come from a third-party library, not the
environment or a pure derivation. This recipe shows the smallest version of
that: a `client` field reads its config from a library call declared in the
`.tono` source itself, via `ext <lib> { extern ... }`. No bespoke code writes
the call, and no lifecycle hook fires it; the field's own construction *is*
the call.

```
service.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
                                                     │
                                                     └── imports ext/{go,ts,rust}/  (the real dependency)
```

Distilled from the appendix of the RFC that introduced this mechanism
(`companyconfig`/`companybus`), scoped down to one library and one call.

## The pieces

- [`service.tono`](service.tono) declares the `ext configlib { ... }` block:
  where the library lives per target (`go:`/`ts:`/`rust:`), the foreign
  return shape per language (`go_config`, `ts_config`, `rust_config`), and
  the real call plus its projection into the logical `app_config` type:

  ```
  extern load(service: string, region: string): app_config {
    go {
      call: "Load"(service, region)
      yields: (cfg: go_config)
      returns: app_config {
        endpoint: match .cfg.Env { "prod" => .cfg.Host, _ => .cfg.DevHost }
        token: .cfg.Token
      }
    }
    ...
  }
  ```

  The `client` entry then constructs its `config` field with that call:

  ```
  config: app_config = configlib.load(.service, .region)
  auth: string @format("Bearer {.config.token}")
  ```

  Nothing outside the `ext` block ever sees `go_config`/`ts_config`/
  `rust_config`; the consumer only sees `app_config`.

- [`ext/go/configlib.go`](ext/go/configlib.go),
  [`ext/ts/configlib/index.ts`](ext/ts/configlib/index.ts), and
  [`ext/rust/src/lib.rs`](ext/rust/src/lib.rs) are stand-ins for the real
  third-party library, one per target, so the generated SDK has something
  real to compile against (the same role `github.com/company/config` plays
  in the RFC's own example). A real integration points the `ext` block at
  the real module/crate/package instead and drops these files.

## The generated tests

The declared test stubs the library call directly, with no library
installed:

```
test "the config from the library becomes the endpoint and the header" {
  stub configlib.load: app_config { endpoint: "https://x.test", token: "t0" }
  c: client { region: "us-east" }
  s: stub c.get_account.http: http.response { ... }
  got: c.get_account()
  expect got: account { ... }
  expect s.requests: [http.request { headers: { "authorization": "Bearer t0", .. }, .. }]
}
```

Go and TypeScript honor the stub by overriding the field's own construction
call outright, so the generated test never reaches the real library. Rust
does not yet implement that override for an `extern`-call field (see the
comment in `ext/rust/src/lib.rs`); its generated test calls the real
stand-in instead, which is why that stand-in is pinned to return the exact
value the stub declares.

## Regenerating

```
frontend compile service.tono --module configsvc > ir.json
tono gen --target go,typescript,rust --out sdk ir.json
```

Each target then needs its dependency pinned at the real library (`go.mod`
`require`, `Cargo.toml` `[dependencies]`, `package.json` `dependencies`) or,
for this recipe, at the stand-in under `ext/` via a local replace/path
dependency — see `scripts/check-example-compiles.sh`'s `config-lib` section
for the exact wiring.

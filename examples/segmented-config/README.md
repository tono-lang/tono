# Segmented endpoint recipe

Some configs come back as `{ <segment>: <endpoint> }`: the consumer picks the
entry for its own segment and falls back to a plain endpoint when the
segment has no override. This recipe shows that shape directly: a field
indexed by another field's value.

```
service.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
```

This recipe is source only; regenerate the SDK yourself with the commands
below.

## The pieces

- [`service.tono`](service.tono) declares the map, the segment key, the
  plain fallback, and the derived field that ties them together:

  ```
  by_segment: map[string]string @env("BY_SEGMENT")
  seg: string @env("SEGMENT") @default("prod")
  endpoint: string @env("ENDPOINT") @default("https://api.example.com")

  segmented_endpoint: string = match .by_segment[.seg] {
    null => .endpoint
    _ => ._
  }
  ```

  Indexing a map by another field's value (`by_segment[.seg]`) resolves to
  an optional value: the key may not be in the map. No target's zero value
  (an empty string, a nil pointer) stands in for that; the match's `null`
  arm is the only way to spell "absent", and it is mandatory whenever the
  subject is optional. The other arm reads the looked-up value back through
  `._`, narrowed to a plain `string` there (the `null` arm has already
  covered absence, so nothing else needs to).

- The generated SDK materializes the lookup idiomatically per target: Go's
  comma-ok (`v, ok := m[k]`), Rust's `Option` via `.get`, TypeScript's
  `!== undefined` check. Read the generated `segmented_endpoint` resolution
  in any target's client to see the shape.

## The generated tests

The two `test` blocks in `service.tono` construct with the segment present
and with it absent, proving both arms resolve without a `ConfigError`
(indexing a map can never fail at runtime, either way). They do not assert
which endpoint the request actually carried: the declared `http.request`
pattern exposes only `method`, `path`, and `body`, not the resolved
endpoint, so there is no declarative way yet to distinguish the two arms'
outcomes from a test. Construction supplies `by_segment`/`seg` directly (a
map literal, same as any other declared test value), so nothing depends on
the environment.

## Regenerating

```
frontend compile service.tono --module segmentedconfig > ir.json
tono gen --target go,typescript,rust --out sdk --go-module example.com/segmentedconfig ir.json
```

# Hybrid SDK: HTTP operations plus bespoke ones

Not every operation an SDK exposes is a request. This example is one client with
three operations: `fetch_note` reaches the service over HTTP, while `save_note`
and `archive_note` are implemented by code you write, bound per language with
`ext impl`.

The rule the typechecker enforces is that every operation an entry declares is
implemented exactly once: a protocol binding (`@http`) or an `ext impl`, never
both and never neither. The generator adds the per-target half: for each
language you generate, a bespoke operation needs a binding in that language, or
it refuses to emit.

```
notes.tono ──frontend──▶ ir.json ──tono gen──▶ SDK
   │
   ├── ext/go/notes.go    (the bespoke half, in Go)
   └── ext/ts/notes.ts    (the bespoke half, in TypeScript)
```

The `test` blocks at the bottom of `notes.tono` are the proof the two halves
agree: the generator emits them as native test files beside each client.

## The two forms

**Typed.** The bound symbol speaks the operation's own types, so it owns the
mapping and reports a declared failure as the declared value:

```go
func StoreSave(ctx context.Context, s *Settings, input Note) (Note, error)
```

Returning `*Overloaded` crosses the boundary untouched, `Retryable()` and all.
Returning anything else becomes `ContractError("save_note", cause)`: the SDK
never leaks an error its caller has no type for.

**Raw.** The bound symbol returns an outcome and the generated glue integrates
it, exactly as it integrates an HTTP response:

```go
func StoreArchive(ctx context.Context, s *Settings, payload []byte) (tonoext.Outcome, error)
```

`Outcome{Success, Code, Body}`. On success the body is decoded strictly into the
declared output (required members present, `DecodeError` carrying the path of
the one that was missing); on failure the `Code` is matched against the
operation's declared `@errorCode`s, falling back to the generic API error. When
the shapes already line up, neither side writes mapping code.

The implementation receives the resolved `Settings`, so an entry field reaches
it with no plumbing: `NOTES_TOKEN` resolves at construction and the bespoke
store reads `s.APIToken` directly.

## Names

`updated_at` is the canonical name and the only one that travels. Go spells it
`Modified` and TypeScript `modified`, because the field carries
`@rename(go: "Modified", typescript: "modified")`. The bespoke implementations
see their own language's spelling; the wire, and therefore the tests, see
`updated_at`.

## Generated tests

An operation implemented in two languages is only trustworthy if something
proves the implementations agree. The `test` blocks in `notes.tono` are that
proof, and the generator will not emit a multi-language `ext impl` without a
test that calls the operation.

Each block constructs the client, optionally stubs one declared dependency,
calls an operation, and asserts the outcome a caller should observe. The
generator emits them as native test files beside the client
(`notes/client_test.go`, `notes/client.test.ts`): no driver to write, one body
of declared cases running under plain `go test` and Vitest in both languages.

Where a test is cut off from the world follows from its stub:

- `stub c.fetch_note.http: http.response { ... }` answers the transport with a
  pinned fixture, and `expect s.requests: [...]` asserts what the SDK actually
  sent: method, path, and headers.
- `stub c.save_note.impl: <typed value or error>` simulates the bespoke
  implementation's outcome, so the test runs without the real store executing.
- No stub at all runs the call against the real dependency. Those land in a
  separate file behind an opt-in marker (`//go:build live`, an env-gated Vitest
  suite), so a default CI run stays hermetic. For this example the "real thing"
  is the deterministic in-repo store, so `scripts/check-impl-conformance.sh`
  runs the live suites too:

```
scripts/check-impl-conformance.sh
```

The tests cover the whole boundary: a normal result, a declared error crossing
typed, an undeclared failure becoming a `ContractError`, an unmatched code
falling back to the generic API error, a response missing a required member, and
a constraint violation caught before the store is ever reached.

## Regenerating

This example is source only (the `.tono`, with its `test` blocks, and the
bespoke halves); the SDK is generated on demand:

```
tono-frontend compile examples/hybrid-notes/notes.tono --module notes > ir.json
tono gen --target go,typescript --out sdk --go-module example.com/notes ir.json
```

Then drop `ext/go/notes.go` into the generated Go package (Go calls a bound
symbol unqualified, from inside the package) and `ext/ts` next to the generated
TypeScript files (the serde file imports it relative to itself). The bound
symbols are named for what they do (`StoreSave`), not for the operation, so the
generated method body reads as a call into the store rather than as recursion.

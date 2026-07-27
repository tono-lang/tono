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
   ├── ext/ts/notes.ts    (the bespoke half, in TypeScript)
   └── vectors/*.json     (proof the two agree)
```

## The two forms

**Typed.** The bound symbol speaks the operation's own types, so it owns the
mapping and reports a declared failure as the declared value:

```go
func SaveNote(ctx context.Context, s *Settings, input Note) (Note, error)
```

Returning `*Overloaded` crosses the boundary untouched, `Retryable()` and all.
Returning anything else becomes `ContractError("save_note", cause)`: the SDK
never leaks an error its caller has no type for.

**Raw.** The bound symbol returns an outcome and the generated glue integrates
it, exactly as it integrates an HTTP response:

```go
func ArchiveNote(ctx context.Context, s *Settings, payload []byte) (tonoext.Outcome, error)
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
see their own language's spelling; the wire, and therefore the vectors, see
`updated_at`.

## Conformance

An operation implemented in two languages is only trustworthy if something
proves the implementations agree. The vectors under `vectors/` are that proof,
and the generator will not emit a multi-language `ext impl` without one.

Each vector case names an input and the outcome a caller should observe. The
drivers under `conformance/` run every case through the *generated client* (not
the bespoke symbol directly, since the glue is part of what the caller sees) and
print the result in one closed vocabulary. `scripts/check-impl-conformance.sh`
runs both and requires that they match each other and the declared expectation.

```
scripts/check-impl-conformance.sh
```

The cases cover the whole boundary: a normal result, a declared error crossing
typed, an undeclared failure becoming a `ContractError`, an unmatched code
falling back to the generic API error, a response missing a required member, and
a constraint violation caught before the store is ever reached.

## Regenerating

This example is source only (the `.tono`, the bespoke halves, and the vectors);
the SDK is generated on demand:

```
tono-frontend compile examples/hybrid-notes/notes.tono --module notes > ir.json
tono gen --target go,typescript --out sdk --go-module example.com/notes ir.json
```

Then drop `ext/go/notes.go` into the generated Go package (Go calls a bound
symbol unqualified, from inside the package) and `ext/ts` next to the generated
TypeScript files (the serde file imports it relative to itself).

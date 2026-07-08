# @tono/http-runtime-ts

The hand-written HTTP transport runtime for tono-generated TypeScript SDKs. It is
a dependency of the generated SDK, not of the compiler, and the only layer that
interprets an operation's opaque wire descriptor.

```
generated SDK  ──execute(descriptor, input, options)──▶  this runtime
```

`execute` reads the descriptor (produced once by the compiler's HTTP Protocol),
builds the request from the bindings (label into the path, query, header, body,
or a single payload as the whole body), performs the `fetch`, and classifies the
response into a raw `Outcome`:

```ts
{ outcome: "success"; status; body } | { outcome: "error"; status; body } | { outcome: "transport"; cause }
```

It raises no typed error. The generated SDK maps the `Outcome` onto its own
idiomatic error taxonomy (a thrown class hierarchy in TypeScript), so every
language handles errors its own way while this runtime stays a thin, neutral
transport.

It ships **no** auth (ADR-0028): bespoke auth sets a header through
`ClientOptions.headers`.

## Develop

```sh
npm ci
npm test        # vitest
npm run typecheck
```

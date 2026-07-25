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

## Transport slots

`ClientOptions` has two mutually exclusive transport slots; setting both is a
construction error (`assertExclusiveTransport`):

- **native**: `fetch` (defaults to the global `fetch`);
- **canonical**: `transport: (req: CanonicalRequest, signal?) => Promise<CanonicalResponse>`,
  for adapting any HTTP stack by mapping plain request/response records instead
  of emulating `fetch`.

The canonical transport contract is **one call, one attempt**: the runtime owns
retry, so a transport with internal retries does not combine with a declared
retry (disable one of the two).

## Retry and timeout

When the descriptor declares retry, an attempt whose outcome is retryable (a
transport failure, or an error response matching a declared retryable error by
status and `code` discriminator) is repeated up to the declared maximum. The
maximum and the per-attempt timeout resolve from a descriptor literal or from a
resolved client field in `ClientOptions.values`. Backoff is a fixed runtime
policy, exponential with full jitter: delay before retry n is
`random() * min(2000ms, 100ms * 2^n)`. The timeout bounds each attempt (the
transport call and body read) separately; a timed-out attempt counts as a
transport failure.

These semantics are pinned by the cross-runtime parity suite in
`../parity/vectors.json`, which every HTTP runtime runs.

## Develop

```sh
npm ci
npm test        # vitest
npm run typecheck
```

# tono http-go runtime

The hand-written HTTP transport runtime for tono-generated Go SDKs. It is a
dependency of the generated SDK, not of the compiler, and the only layer that
interprets an operation's opaque wire descriptor.

```
generated SDK  ──runtime.Execute(ctx, descriptor, input, hooks)──▶  this runtime
```

`Execute` reads the descriptor (produced once by the compiler's HTTP Protocol),
builds the request from the bindings (label into the path, query, header, body,
or a single payload as the whole body), performs the call, and classifies the
response into a raw `Outcome` (`success`, `error`, or `transport`). It raises
no typed error: the generated SDK maps the `Outcome` onto its own idiomatic
taxonomy (an `error` return in Go). The returned `error` is reserved for hook
failures and unencodable input, which propagate raw.

It ships **no** auth (ADR-0028): bespoke auth sets a header through
`Options.Headers` or a hook.

## Transport slots

`Options` has two mutually exclusive transport slots; `New` fails when both are
set:

- **native**: `Client *http.Client` (defaults to `http.DefaultClient`);
- **canonical**: `Transport func(ctx, CanonicalRequest) (CanonicalResponse, error)`,
  for adapting any HTTP stack by mapping plain request/response records instead
  of emulating `*http.Client`.

The canonical transport contract is **one call, one attempt**: the runtime owns
retry, so a transport with internal retries does not combine with a declared
retry (disable one of the two). The transport must honor ctx cancellation; the
per-attempt timeout arrives as a ctx deadline.

## Retry and timeout

When the descriptor declares retry, an attempt whose outcome is retryable (a
transport failure, or an error response matching a declared retryable error by
status and `code` discriminator) is repeated up to the declared maximum. The
maximum and the per-attempt timeout resolve from a descriptor literal or from a
resolved client field in `Options.Values`. Backoff is a fixed runtime policy,
exponential with full jitter: delay before retry n is
`random() * min(2000ms, 100ms * 2^n)`. The timeout bounds each attempt (the
transport call and body read) separately, starting after the `BeforeRequest`
hook; a timed-out attempt counts as a transport failure.

These semantics are pinned by the cross-runtime parity suite in
`../parity/vectors.json`, which every HTTP runtime runs.

## Develop

```sh
go test ./...
gremlins unleash   # mutation gate; thresholds in .gremlins.yaml
```

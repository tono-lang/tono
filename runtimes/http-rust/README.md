# tono http-rust runtime

The hand-written HTTP transport runtime for tono-generated Rust SDKs. It is a
dependency of the generated SDK, not of the compiler, and the only layer that
interprets an operation's opaque wire descriptor.

```
generated SDK  ──runtime.execute(descriptor, input, hooks).await──▶  this runtime
```

`Runtime::execute` reads the descriptor (produced once by the compiler's HTTP
Protocol), builds the request from the bindings (label into the path, query,
header, body, or a single payload as the whole body), performs the call, and
classifies the response into a raw `Outcome` (`Success`, `Error`, or
`Transport`). It raises no typed error: the generated SDK maps the `Outcome`
onto its own idiomatic taxonomy (`TonoError`). The returned `Result::Err` is
reserved for hook failures, which propagate raw.

It ships **no** auth (ADR-0028): bespoke auth sets a header through
`Options::headers` or a hook.

## Transport slots

`Options` has two mutually exclusive transport slots; `Runtime::new` fails
when both are set:

- **native**: `client: Option<reqwest::Client>` (defaults to a fresh
  `reqwest::Client`);
- **canonical**: `transport: Option<Transport>`, a boxed async closure
  `Fn(CanonicalRequest) -> Result<CanonicalResponse, ExecuteError>`, for
  adapting any HTTP stack by mapping plain request/response records instead
  of driving `reqwest::Client` directly.

The canonical transport contract is **one call, one attempt**: the runtime
owns retry, so a transport with internal retries does not combine with a
declared retry (disable one of the two). Cancellation is Rust's native one:
`execute` holds no state across an `.await` that outlives its own future, so
wrapping the call in `tokio::time::timeout` or racing it in `tokio::select!`
and dropping it cancels cleanly, at any await point, with no separate
cancellation token to thread through.

## Retry and timeout

When the descriptor declares retry, an attempt whose outcome is retryable (a
transport failure, or an error response matching a declared retryable error by
status and `code` discriminator) is repeated up to the declared maximum. The
maximum and the per-attempt timeout resolve from a descriptor literal or from
a resolved client field in `Options::values`. Backoff is a fixed runtime
policy, exponential with full jitter: delay before retry n is
`random() * min(2000ms, 100ms * 2^n)`. The timeout bounds each attempt (the
transport call and body read) separately, starting after the `before_request`
hook; a timed-out attempt counts as a transport failure.

These semantics are pinned by the cross-runtime parity suite in
`../parity/vectors.json`, which every HTTP runtime runs.

## Develop

```sh
cargo test
cargo clippy -- -D warnings
cargo mutants   # mutation gate; scope in .cargo/mutants.toml
```

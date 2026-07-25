// The hand-written HTTP runtime for tono-generated TypeScript SDKs. Small,
// stable, and the only layer that interprets a wire descriptor. It ships no auth
// of its own; bespoke auth sets a header through `ClientOptions.headers`.

export type {
  CanonicalRequest,
  CanonicalResponse,
  CanonicalTransport,
  ClientOptions,
  Hooks,
  Outcome,
  Part,
  ResponsePart,
  RetrySpec,
  ValueSource,
  WireDescriptor,
} from "./descriptor";
export type { ExecuteSeams } from "./execute";
export { assertExclusiveTransport, execute } from "./execute";

// The hand-written HTTP runtime for tono-generated TypeScript SDKs. Small,
// stable, and the only layer that interprets a wire descriptor. It ships no auth
// of its own; bespoke auth sets a header through `ClientOptions.headers`.

export type {
  CanonicalRequest,
  CanonicalResponse,
  ClientOptions,
  Hooks,
  Outcome,
  Part,
  ResponsePart,
  WireDescriptor,
} from "./descriptor";
export { execute } from "./execute";

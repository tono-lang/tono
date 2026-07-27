// The wire descriptor: the opaque, language-agnostic shape the tono HTTP
// Protocol produces for each operation and the generated SDK embeds verbatim.
// This runtime is the only layer that reads it. The field names mirror the JSON
// the compiler emits (snake_case), so a generated `JSON.parse(...)` literal is a
// `WireDescriptor` with no transformation.

// Where one input member goes in the HTTP request.
export type Part =
  | { kind: "label" } // substitutes {member-name} in the uri
  | { kind: "query"; name: string } // query-string parameter
  | { kind: "header"; name: string } // request header
  | { kind: "body" } // a field inside the JSON body (default)
  | { kind: "payload" }; // this member is the whole body, no envelope

// Where one output member is read from in the HTTP response.
export type ResponsePart = { kind: "header"; name: string } | { kind: "statusCode" };

// A descriptor position that yields a number at call time: a literal frozen by
// the compiler, or a reference to a resolved client field looked up in
// `ClientOptions.values` by canonical name.
export type ValueSource = { readonly ref: string } | { readonly lit: number };

// One piece of a template position: a literal run, an entry-field placeholder
// resolved from `ClientOptions.values` by its dotted path, or an
// operation-input placeholder resolved from the call's input record.
export type TemplatePart =
  | { readonly lit: string }
  | { readonly field: ReadonlyArray<string> }
  | { readonly input: string };

// A descriptor value position: a literal frozen by the compiler, an
// entry-field reference resolved from `ClientOptions.values`, or a template of
// parts.
export type ValueExpr =
  | { readonly lit: unknown }
  | { readonly field: ReadonlyArray<string> }
  | { readonly template: ReadonlyArray<TemplatePart> };

// Declares that the operation retries, with the maximum number of retries
// (attempts after the first) read from `max`.
export interface RetrySpec {
  readonly max: ValueSource;
}

export interface WireDescriptor {
  readonly http_method: string;
  readonly uri: string;
  readonly bindings: ReadonlyArray<readonly [string, Part]>;
  readonly response_bindings: ReadonlyArray<readonly [string, ResponsePart]>;
  // status -> output type ref (opaque here; the SDK owns decoding). The runtime
  // uses only the status to decide success vs error.
  readonly success: ReadonlyArray<readonly [number, unknown]>;
  // status -> error shape id -> optional @errorCode discriminator value ->
  // whether the error is retryable. The SDK's discriminator consumes the id;
  // the runtime consumes status, code, and the retryable flag (absent in
  // descriptors emitted before retry existed, meaning not retryable).
  readonly errors: ReadonlyArray<readonly [number, string, string | null, boolean?]>;
  // Absent retry means one attempt, ever. `timeout` is the per-attempt budget
  // in milliseconds; absent means no per-attempt deadline.
  readonly retry?: RetrySpec | null;
  readonly timeout?: ValueSource | null;
  // `endpoint` names the resolved client field (by path) whose value is the
  // base URL for this operation; absent means `ClientOptions.baseUrl`.
  // `request_headers` are the operation's declared headers (key template ->
  // value expression), applied before the caller's `headers` so a bespoke or
  // caller-supplied header wins.
  readonly endpoint?: ReadonlyArray<string> | null;
  readonly request_headers?: ReadonlyArray<readonly [ReadonlyArray<TemplatePart>, ValueExpr]> | null;
}

// The canonical transport slot: adapt any HTTP stack by mapping
// CanonicalRequest/CanonicalResponse, without emulating `fetch`.
//
// Contract: one call is one attempt. The runtime owns retry (driven by the
// descriptor's retry declaration), so a transport with internal retries does
// not combine with it: either disable the transport's retries or do not
// declare retry on the operations. The signal fires when the per-attempt
// timeout expires; a transport should abort its work on it (the runtime times
// the attempt out regardless, but cannot reclaim what the transport started).
export type CanonicalTransport = (
  req: CanonicalRequest,
  signal?: AbortSignal,
) => Promise<CanonicalResponse>;

// What the caller supplies once, shared across every operation. Exactly one
// transport slot may be set: `fetch` (native) or `transport` (canonical);
// setting both is a construction error. The runtime ships no auth of its own;
// a bespoke hook sets an auth header through `headers`.
export interface ClientOptions {
  readonly baseUrl: string;
  readonly fetch?: typeof fetch;
  readonly transport?: CanonicalTransport;
  readonly headers?: Readonly<Record<string, string>>;
  // The resolved client fields the descriptor's ref positions (retry max,
  // timeout) look up by canonical name. The runtime stays blind to where they
  // came from.
  readonly values?: Readonly<Record<string, unknown>>;
}

// The request the runtime builds from a descriptor before sending it. A
// `before_request` hook receives this and may return a mutated copy (set an auth
// header, sign the body). Exposed so a bespoke hook can read and rewrite the
// request without reaching into protocol internals.
export interface CanonicalRequest {
  method: string;
  url: string;
  headers: Record<string, string>;
  body: string | undefined;
}

// The response the runtime reads before classifying it. An `after_response` hook
// may return a mutated copy.
export interface CanonicalResponse {
  status: number;
  headers: Record<string, string>;
  body: string;
}

// The lifecycle slots the runtime invokes around the transport. `client_init`
// and `on_error` are applied by the generated client (they touch client
// construction and the error taxonomy), so they are not part of this interface.
// The runtime never wraps a throw: a hook that throws propagates raw, and the
// generated wrapper is what turns it into a ContractError.
export interface Hooks {
  readonly before_request?: (
    req: CanonicalRequest,
  ) => CanonicalRequest | Promise<CanonicalRequest>;
  readonly after_response?: (
    res: CanonicalResponse,
  ) => CanonicalResponse | Promise<CanonicalResponse>;
}

// The raw result of one call. Deliberately free of any error class: the
// generated SDK maps each variant onto its own idiomatic taxonomy (throw a typed
// error in TypeScript, a `Result` in Rust, an `error` return in Go), so the
// runtime stays a thin, language-neutral transport.
export type Outcome =
  | { readonly outcome: "success"; readonly status: number; readonly body: string }
  | { readonly outcome: "error"; readonly status: number; readonly body: string }
  | { readonly outcome: "transport"; readonly cause: unknown };

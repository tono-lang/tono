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

export interface WireDescriptor {
  readonly http_method: string;
  readonly uri: string;
  readonly bindings: ReadonlyArray<readonly [string, Part]>;
  readonly response_bindings: ReadonlyArray<readonly [string, ResponsePart]>;
  // status -> output type ref (opaque here; the SDK owns decoding). The runtime
  // uses only the status to decide success vs error.
  readonly success: ReadonlyArray<readonly [number, unknown]>;
  // status -> error shape id -> optional @errorCode body field (unused by the
  // runtime; the SDK's discriminator consumes it).
  readonly errors: ReadonlyArray<readonly [number, string, string | null]>;
}

// What the caller supplies once, shared across every operation. The runtime
// ships no auth (ADR-0028); a bespoke hook sets an auth header through `headers`.
export interface ClientOptions {
  readonly baseUrl: string;
  readonly fetch?: typeof fetch;
  readonly headers?: Readonly<Record<string, string>>;
}

// The raw result of one call. Deliberately free of any error class: the
// generated SDK maps each variant onto its own idiomatic taxonomy (throw a typed
// error in TypeScript, a `Result` in Rust, an `error` return in Go), so the
// runtime stays a thin, language-neutral transport.
export type Outcome =
  | { readonly outcome: "success"; readonly status: number; readonly body: string }
  | { readonly outcome: "error"; readonly status: number; readonly body: string }
  | { readonly outcome: "transport"; readonly cause: unknown };

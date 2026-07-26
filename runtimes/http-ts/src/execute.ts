// The transport core: interpret a wire descriptor, build the request, perform
// the call, and classify the response into a raw Outcome. This is the single
// layer where protocol (HTTP) and language (TypeScript) meet.
//
// The descriptor is executed verbatim: every binding was resolved once by the
// Protocol and frozen into the descriptor, so nothing here re-derives a default
// (an unmarked member is already a `body` entry, a label is already a `label`
// entry). The runtime raises no typed error; the generated SDK maps the Outcome
// onto its taxonomy.
//
// When the descriptor declares retry, an attempt whose outcome is retryable (a
// transport failure, or an error response matching a declared retryable error)
// is repeated up to the declared maximum, with exponential full-jitter backoff
// between attempts. The declared timeout bounds each attempt separately.

import type {
  CanonicalRequest,
  CanonicalResponse,
  ClientOptions,
  Hooks,
  Outcome,
  TemplatePart,
  ValueExpr,
  WireDescriptor,
} from "./descriptor";
import { backoffDelayMs, isRetryable, resolveMaxRetries, resolveTimeoutMs } from "./retry";

// The encoded wire record the generated SDK passes in: member wire-name -> value.
type Input = Record<string, unknown>;

// Deterministic seams for the retry loop: production uses the real clock and
// Math.random; tests pin them to make retry timing and jitter assertable.
export interface ExecuteSeams {
  readonly sleep?: (ms: number) => Promise<void>;
  readonly random?: () => number;
}

// Setting both transport slots is a construction error: the caller must pick
// the native slot (`fetch`) or the canonical one (`transport`). Exported so a
// generated client constructor can fail at construction, not at first call.
export function assertExclusiveTransport(options: ClientOptions): void {
  if (options.fetch && options.transport) {
    throw new Error(
      "ClientOptions.fetch and ClientOptions.transport are mutually exclusive: set the native slot or the canonical slot, not both",
    );
  }
}

function asRecord(input: unknown): Input {
  return input !== null && typeof input === "object" ? (input as Input) : {};
}

// The resolved client values the descriptor's ref positions look up.
type Values = Readonly<Record<string, unknown>>;

// Render a scalar the way the wire expects it in a path, query, or header
// position: strings verbatim, numbers and booleans in their canonical JS form,
// anything else as JSON (matching the other runtimes).
function formatScalar(value: unknown): string {
  if (typeof value === "string") return value;
  // JSON is already the canonical spelling for numbers and booleans; anything
  // unserializable renders empty rather than failing the request line.
  try {
    return JSON.stringify(value) ?? "";
  } catch {
    return "";
  }
}

// Substitute each label binding into its `{name}` placeholder and each
// `{.field}` placeholder with the resolved client value under that dotted path
// in `values`. A path parameter must be present, so an absent one substitutes
// empty rather than the literal "undefined"/"null".
function buildPath(descriptor: WireDescriptor, record: Input, values: Values): string {
  let path = descriptor.uri;
  for (const [name, part] of descriptor.bindings) {
    if (part.kind !== "label") continue;
    const value = record[name];
    path = path.replace(
      `{${name}}`,
      value === undefined || value === null ? "" : encodeURIComponent(String(value)),
    );
  }
  return substituteFieldPlaceholders(path, values);
}

// Replace every `{.a.b}` run in a path template with the value under "a.b" in
// the resolved client values, escaped. An absent or null value substitutes
// empty, the same rule as an absent label; an unterminated `{.` stays verbatim.
function substituteFieldPlaceholders(path: string, values: Values): string {
  return path.replace(/\{\.([^}]*)\}/g, (_, key: string) => {
    return encodeURIComponent(formatScalar(values[key] ?? ""));
  });
}

// The operation's base URL from its endpoint field reference: the string value
// under the dotted path in the resolved client values. An absent declaration,
// absent value, or non-string falls back to `ClientOptions.baseUrl`.
function resolveEndpoint(descriptor: WireDescriptor, values: Values): string | undefined {
  const endpoint = descriptor.endpoint;
  if (!endpoint?.length) return undefined;
  const value = values[endpoint.join(".")];
  return typeof value === "string" && value !== "" ? value : undefined;
}

// Render template parts into one string: literal runs verbatim, entry-field
// parts from the resolved client values, input parts from the call's input
// record. Undefined when any field/input part has no value (the caller omits
// the position rather than emitting a half-rendered one).
function resolveTemplate(
  parts: ReadonlyArray<TemplatePart>,
  values: Values,
  record: Input,
): string | undefined {
  let out = "";
  for (const part of parts) {
    if ("lit" in part) {
      out += part.lit;
      continue;
    }
    const value = "field" in part ? values[part.field.join(".")] : record[part.input];
    if (value === undefined || value === null) return undefined;
    out += formatScalar(value);
  }
  return out;
}

// Render a descriptor value position: a literal verbatim, a field reference
// from the resolved client values, or a template. Undefined when the
// referenced value is absent.
function resolveValueExpr(expr: ValueExpr, values: Values, record: Input): string | undefined {
  if ("field" in expr) {
    const value = values[expr.field.join(".")];
    return value === undefined || value === null ? undefined : formatScalar(value);
  }
  if ("template" in expr) return resolveTemplate(expr.template, values, record);
  return expr.lit === undefined || expr.lit === null ? undefined : formatScalar(expr.lit);
}

// Resolve the operation's declared request headers. A header whose key or
// value cannot resolve is omitted whole: half a header is worse than none, and
// the declared chain's absence rules already ran in generated code.
function declaredHeaders(
  descriptor: WireDescriptor,
  values: Values,
  record: Input,
): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const [key, expr] of descriptor.request_headers ?? []) {
    const name = resolveTemplate(key, values, record);
    if (name === undefined || name === "") continue;
    const value = resolveValueExpr(expr, values, record);
    if (value === undefined) continue;
    headers[name] = value;
  }
  return headers;
}

// A query value serializes as a repeated entry per element for a list, a single
// entry otherwise; a null/absent value is omitted (the body's nullable-omit rule,
// applied to the request line).
function buildQuery(descriptor: WireDescriptor, record: Input): string {
  const query = new URLSearchParams();
  for (const [name, part] of descriptor.bindings) {
    if (part.kind !== "query") continue;
    const value = record[name];
    if (value === undefined || value === null) continue;
    if (Array.isArray(value)) {
      for (const element of value) query.append(part.name, String(element));
    } else {
      query.append(part.name, String(value));
    }
  }
  return query.toString();
}

// Layer the request headers: the operation's declared headers first, the
// caller's base headers over them (a bespoke or caller-supplied header wins
// over a declared one), and the input's header bindings last (the per-call
// value is the most specific). A `before_request` hook sees the fully layered
// result.
function buildHeaders(
  descriptor: WireDescriptor,
  record: Input,
  base: Readonly<Record<string, string>>,
  values: Values,
): Record<string, string> {
  const headers: Record<string, string> = declaredHeaders(descriptor, values, record);
  for (const [name, value] of Object.entries(base)) setHeader(headers, name, value);
  for (const [name, part] of descriptor.bindings) {
    if (part.kind !== "header") continue;
    const value = record[name];
    if (value !== undefined && value !== null) setHeader(headers, part.name, String(value));
  }
  return headers;
}

// Override across casings: header names are case-insensitive, so a bespoke
// "authorization" must replace a declared "Authorization" rather than ride
// beside it.
function setHeader(headers: Record<string, string>, name: string, value: string): void {
  const lower = name.toLowerCase();
  for (const key of Object.keys(headers)) {
    if (key.toLowerCase() === lower) delete headers[key];
  }
  headers[name] = value;
}

// The request body: a single @httpPayload member as the whole body, otherwise
// the body members assembled into a JSON object (or nothing when there are none).
function buildBody(descriptor: WireDescriptor, record: Input): string | undefined {
  const bodyFields: Record<string, unknown> = {};
  for (const [name, part] of descriptor.bindings) {
    if (part.kind === "payload") return JSON.stringify(record[name]);
    if (part.kind === "body" && record[name] !== undefined) bodyFields[name] = record[name];
  }
  return Object.keys(bodyFields).length > 0 ? JSON.stringify(bodyFields) : undefined;
}

// HTTP header names are case-insensitive, so a caller-supplied "Content-Type"
// must suppress the default rather than sit beside a second "content-type".
function hasHeader(headers: Record<string, string>, name: string): boolean {
  const lower = name.toLowerCase();
  return Object.keys(headers).some((key) => key.toLowerCase() === lower);
}

// Collect a response's headers into a plain record so a hook can read and
// rewrite them without a live `Headers` object.
function headerRecord(headers: Headers): Record<string, string> {
  const record: Record<string, string> = {};
  headers.forEach((value, key) => {
    record[key] = value;
  });
  return record;
}

// Read the response-bound members off the (canonical) response (a header value,
// or the status code) and fold them into the decoded body so the generated
// decoder sees them as ordinary fields. Applied only on success; interpreting
// the descriptor is the runtime's job, which keeps the generated client blind.
function applyResponseBindings(descriptor: WireDescriptor, res: CanonicalResponse): string {
  if (descriptor.response_bindings.length === 0) return res.body;
  let object: Record<string, unknown> = {};
  // Equivalent-mutant guard: the empty-body check only skips a JSON.parse("")
  // that the catch below would swallow anyway, so mutating it cannot change the
  // resulting `{}`. It stays for intent (do not parse an empty body).
  // Stryker disable next-line ConditionalExpression,StringLiteral
  if (res.body !== "") {
    try {
      object = JSON.parse(res.body) as Record<string, unknown>;
    } catch {
      // A non-object body leaves the response-bound fields to stand on their own.
    }
  }
  for (const [member, part] of descriptor.response_bindings) {
    // Header names are case-insensitive; `headerRecord` lowercases every key.
    object[member] =
      part.kind === "statusCode" ? res.status : (res.headers[part.name.toLowerCase()] ?? null);
  }
  return JSON.stringify(object);
}

// A 2xx is a success even when its exact code was not the one declared (a server
// may answer 200 where 201 was declared, or 204 with no body); a declared success
// code outside the 2xx range still counts.
function isSuccessStatus(descriptor: WireDescriptor, status: number): boolean {
  if (status >= 200 && status < 300) return true;
  return descriptor.success.some(([declared]) => declared === status);
}

async function nativeCall(
  request: CanonicalRequest,
  options: ClientOptions,
  signal: AbortSignal | undefined,
): Promise<CanonicalResponse> {
  const transport = options.fetch ?? fetch;
  const response = await transport(request.url, {
    method: request.method,
    headers: request.headers,
    body: request.body,
    signal,
  });
  // The body read can fail mid-stream too, so it shares the transport error path.
  const text = await response.text();
  return { status: response.status, headers: headerRecord(response.headers), body: text };
}

// One transport call under the per-attempt timeout. The timeout aborts the
// signal (so a cooperating transport cancels its work) and rejects the attempt
// regardless, so a transport that ignores the signal still times out.
async function callTransport(
  request: CanonicalRequest,
  options: ClientOptions,
  timeoutMs: number,
): Promise<CanonicalResponse> {
  if (timeoutMs <= 0) {
    return options.transport
      ? options.transport(request, undefined)
      : nativeCall(request, options, undefined);
  }
  const controller = new AbortController();
  let timer: ReturnType<typeof setTimeout> | undefined;
  const expiry = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      const cause = new Error(`attempt timed out after ${timeoutMs}ms`);
      controller.abort(cause);
      reject(cause);
    }, timeoutMs);
  });
  const call = options.transport
    ? options.transport(request, controller.signal)
    : nativeCall(request, options, controller.signal);
  try {
    const raced = Promise.race([call, expiry]);
    // The losing branch settles later; mark it handled so its rejection does
    // not surface as an unhandled one.
    call.catch(() => {});
    return await raced;
  } finally {
    clearTimeout(timer);
  }
}

// One attempt: build the request, run the hooks, call the transport, classify.
async function attemptCall(
  descriptor: WireDescriptor,
  record: Input,
  options: ClientOptions,
  hooks: Hooks | undefined,
  timeoutMs: number,
): Promise<Outcome> {
  const values = options.values ?? {};
  const headers = buildHeaders(descriptor, record, options.headers ?? {}, values);
  const body = buildBody(descriptor, record);
  if (body !== undefined && !hasHeader(headers, "content-type")) {
    headers["content-type"] = "application/json";
  }
  const qs = buildQuery(descriptor, record);
  const base = resolveEndpoint(descriptor, values) ?? options.baseUrl;
  const url = base + buildPath(descriptor, record, values) + (qs ? `?${qs}` : "");

  // The bespoke `before_request` hook runs outside the transport try/catch: a
  // hook that throws must surface as its own error (the generated wrapper turns
  // it into a ContractError), not be misreported as a transport failure. The
  // timeout covers the transport call and body read only, starting after the
  // hook so a slow hook does not eat into the attempt.
  let request: CanonicalRequest = { method: descriptor.http_method, url, headers, body };
  if (hooks?.before_request) request = await hooks.before_request(request);

  let canonical: CanonicalResponse;
  try {
    canonical = await callTransport(request, options, timeoutMs);
  } catch (cause) {
    return { outcome: "transport", cause };
  }

  // `after_response` likewise runs outside the catch so its throw propagates.
  if (hooks?.after_response) canonical = await hooks.after_response(canonical);

  if (isSuccessStatus(descriptor, canonical.status)) {
    return {
      outcome: "success",
      status: canonical.status,
      body: applyResponseBindings(descriptor, canonical),
    };
  }
  return { outcome: "error", status: canonical.status, body: canonical.body };
}

function defaultSleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

export async function execute(
  descriptor: WireDescriptor,
  input: unknown,
  options: ClientOptions,
  hooks?: Hooks,
  seams?: ExecuteSeams,
): Promise<Outcome> {
  assertExclusiveTransport(options);
  const record = asRecord(input);
  const maxRetries = resolveMaxRetries(descriptor.retry, options.values);
  const timeoutMs = resolveTimeoutMs(descriptor.timeout, options.values);
  const random = seams?.random ?? Math.random;
  const sleep = seams?.sleep ?? defaultSleep;
  for (let attempt = 0; ; attempt++) {
    const outcome = await attemptCall(descriptor, record, options, hooks, timeoutMs);
    if (attempt >= maxRetries || !isRetryable(descriptor, outcome)) return outcome;
    await sleep(backoffDelayMs(attempt, random()));
  }
}

// The transport core: interpret a wire descriptor, build the request, perform
// the call, and classify the response into a raw Outcome. This is the single
// layer where protocol (HTTP) and language (TypeScript) meet.
//
// The descriptor is executed verbatim: every binding was resolved once by the
// Protocol and frozen into the descriptor, so nothing here re-derives a default
// (an unmarked member is already a `body` entry, a label is already a `label`
// entry). The runtime raises no typed error; the generated SDK maps the Outcome
// onto its taxonomy.

import type { ClientOptions, Outcome, WireDescriptor } from "./descriptor";

// The encoded wire record the generated SDK passes in: member wire-name -> value.
type Input = Record<string, unknown>;

function asRecord(input: unknown): Input {
  return input !== null && typeof input === "object" ? (input as Input) : {};
}

// Substitute each label binding into its `{name}` placeholder. A path parameter
// must be present, so an absent one substitutes empty rather than the literal
// "undefined"/"null".
function buildPath(descriptor: WireDescriptor, record: Input): string {
  let path = descriptor.uri;
  for (const [name, part] of descriptor.bindings) {
    if (part.kind !== "label") continue;
    const value = record[name];
    path = path.replace(
      `{${name}}`,
      value === undefined || value === null ? "" : encodeURIComponent(String(value)),
    );
  }
  return path;
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

function buildHeaders(
  descriptor: WireDescriptor,
  record: Input,
  base: Readonly<Record<string, string>>,
): Record<string, string> {
  const headers: Record<string, string> = { ...base };
  for (const [name, part] of descriptor.bindings) {
    if (part.kind !== "header") continue;
    const value = record[name];
    if (value !== undefined && value !== null) headers[part.name] = String(value);
  }
  return headers;
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

// Read the response-bound members off the response (a header value, or the
// status code) and fold them into the decoded body so the generated decoder sees
// them as ordinary fields. Applied only on success; interpreting the descriptor
// is the runtime's job, which keeps the generated client blind to it.
function applyResponseBindings(
  descriptor: WireDescriptor,
  response: Response,
  text: string,
): string {
  if (descriptor.response_bindings.length === 0) return text;
  let object: Record<string, unknown> = {};
  // Equivalent-mutant guard: the empty-text check only skips a JSON.parse("")
  // that the catch below would swallow anyway, so mutating it cannot change the
  // resulting `{}`. It stays for intent (do not parse an empty body).
  // Stryker disable next-line ConditionalExpression,StringLiteral
  if (text !== "") {
    try {
      object = JSON.parse(text) as Record<string, unknown>;
    } catch {
      // A non-object body leaves the response-bound fields to stand on their own.
    }
  }
  for (const [member, part] of descriptor.response_bindings) {
    object[member] =
      part.kind === "statusCode" ? response.status : response.headers.get(part.name);
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

export async function execute(
  descriptor: WireDescriptor,
  input: unknown,
  options: ClientOptions,
): Promise<Outcome> {
  const record = asRecord(input);

  const headers = buildHeaders(descriptor, record, options.headers ?? {});
  const body = buildBody(descriptor, record);
  if (body !== undefined && !hasHeader(headers, "content-type")) {
    headers["content-type"] = "application/json";
  }
  const qs = buildQuery(descriptor, record);
  const url = options.baseUrl + buildPath(descriptor, record) + (qs ? `?${qs}` : "");

  const transport = options.fetch ?? fetch;
  let response: Response;
  let text: string;
  try {
    response = await transport(url, { method: descriptor.http_method, headers, body });
    // The body read can fail mid-stream too, so it shares the transport catch.
    text = await response.text();
  } catch (cause) {
    return { outcome: "transport", cause };
  }

  if (isSuccessStatus(descriptor, response.status)) {
    return {
      outcome: "success",
      status: response.status,
      body: applyResponseBindings(descriptor, response, text),
    };
  }
  return { outcome: "error", status: response.status, body: text };
}

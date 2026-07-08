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

// A query/header value serializes as a repeated entry per element for a list, a
// single entry otherwise; a null/absent value is omitted (the body's
// nullable-omit rule, applied to the request line).
function appendQuery(query: URLSearchParams, name: string, value: unknown): void {
  if (value === undefined || value === null) return;
  if (Array.isArray(value)) {
    for (const element of value) query.append(name, String(element));
  } else {
    query.append(name, String(value));
  }
}

// HTTP header names are case-insensitive, so a caller-supplied "Content-Type"
// must suppress the default rather than sit beside a second "content-type".
function hasHeader(headers: Record<string, string>, name: string): boolean {
  const lower = name.toLowerCase();
  return Object.keys(headers).some((key) => key.toLowerCase() === lower);
}

// Read the response-bound members off the response (a header value, or the
// status code) and fold them into the decoded body so the generated decoder
// sees them as ordinary fields. Applied only on success; interpreting the
// descriptor is the runtime's job, which keeps the generated client blind to it.
function applyResponseBindings(
  descriptor: WireDescriptor,
  response: Response,
  text: string,
): string {
  if (descriptor.response_bindings.length === 0) return text;
  let object: Record<string, unknown> = {};
  if (text !== "") {
    try {
      object = JSON.parse(text) as Record<string, unknown>;
    } catch {
      object = {};
    }
  }
  for (const [member, part] of descriptor.response_bindings) {
    object[member] =
      part.kind === "statusCode" ? response.status : response.headers.get(part.name);
  }
  return JSON.stringify(object);
}

// A 2xx is a success even when its exact code was not the one declared (a server
// may answer 200 where 201 was declared, or 204 with no body); a declared
// success code outside the 2xx range still counts.
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

  let path = descriptor.uri;
  const query = new URLSearchParams();
  const headers: Record<string, string> = { ...(options.headers ?? {}) };
  const bodyFields: Record<string, unknown> = {};
  let payload: unknown;
  let hasPayload = false;

  for (const [name, part] of descriptor.bindings) {
    const value = record[name];
    switch (part.kind) {
      case "label":
        // A path parameter must be present; a missing one substitutes empty
        // rather than the literal "undefined"/"null".
        path = path.replace(
          `{${name}}`,
          value === undefined || value === null ? "" : encodeURIComponent(String(value)),
        );
        break;
      case "query":
        appendQuery(query, part.name, value);
        break;
      case "header":
        if (value !== undefined && value !== null) headers[part.name] = String(value);
        break;
      case "payload":
        hasPayload = true;
        payload = value;
        break;
      case "body":
        if (value !== undefined) bodyFields[name] = value;
        break;
    }
  }

  const qs = query.toString();
  const url = options.baseUrl + path + (qs ? `?${qs}` : "");

  let body: string | undefined;
  if (hasPayload) {
    body = JSON.stringify(payload);
  } else if (Object.keys(bodyFields).length > 0) {
    body = JSON.stringify(bodyFields);
  }
  if (body !== undefined && !hasHeader(headers, "content-type")) {
    headers["content-type"] = "application/json";
  }

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

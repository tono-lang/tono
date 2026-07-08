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
        path = path.replace(`{${name}}`, encodeURIComponent(String(value)));
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
  if (body !== undefined && headers["content-type"] === undefined) {
    headers["content-type"] = "application/json";
  }

  const transport = options.fetch ?? fetch;
  let response: Response;
  try {
    response = await transport(url, { method: descriptor.http_method, headers, body });
  } catch (cause) {
    return { outcome: "transport", cause };
  }

  const text = await response.text();
  const isSuccess = descriptor.success.some(([status]) => status === response.status);
  return isSuccess
    ? { outcome: "success", status: response.status, body: text }
    : { outcome: "error", status: response.status, body: text };
}

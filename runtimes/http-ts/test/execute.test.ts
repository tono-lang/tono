import { describe, expect, it } from "vitest";

import { execute } from "../src/execute";
import type { Part, WireDescriptor } from "../src/descriptor";

// A recording transport: captures the one request it is given and returns a
// canned response, so a test can assert exactly what the runtime built.
function recorder(status = 200, body = "{}", headers: Record<string, string> = {}) {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetchImpl = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    return new Response(body, { status, headers });
  }) as unknown as typeof fetch;
  return { calls, fetchImpl };
}

function descriptor(over: Partial<WireDescriptor> = {}): WireDescriptor {
  return {
    http_method: "POST",
    uri: "/things",
    bindings: [],
    response_bindings: [],
    success: [[200, null]],
    errors: [],
    ...over,
  };
}

const binding = (name: string, part: Part): readonly [string, Part] => [name, part];

describe("request building across the five part variants", () => {
  it("substitutes a label into the uri path", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things/{id}", bindings: [binding("id", { kind: "label" })] }),
      { id: "abc" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/abc");
  });

  it("substitutes an absent label as empty, never the literal undefined", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things/{id}", bindings: [binding("id", { kind: "label" })] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/");
  });

  it("appends query members, repeating a list", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        http_method: "GET",
        uri: "/things",
        bindings: [binding("q", { kind: "query", name: "q" }), binding("tag", { kind: "query", name: "tag" })],
      }),
      { q: "hi", tag: ["a", "b"] },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things?q=hi&tag=a&tag=b");
  });

  it("omits a null query member", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things", bindings: [binding("q", { kind: "query", name: "q" })] }),
      { q: null },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things");
  });

  it("sets a header member", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("trace", { kind: "header", name: "X-Trace" })] }),
      { trace: "t1" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect((calls[0].init.headers as Record<string, string>)["X-Trace"]).toBe("t1");
  });

  it("assembles body members into a JSON object", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" }), binding("b", { kind: "body" })] }),
      { a: 1, b: "two" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].init.body).toBe(JSON.stringify({ a: 1, b: "two" }));
    expect((calls[0].init.headers as Record<string, string>)["content-type"]).toBe("application/json");
  });

  it("sends the single payload member as the whole body, no envelope", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("raw", { kind: "payload" })] }),
      { raw: { nested: true } },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].init.body).toBe(JSON.stringify({ nested: true }));
  });

  it("does not add a second content-type when the caller set one under a different case", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" })] }),
      { a: 1 },
      { baseUrl: "https://api.test", fetch: fetchImpl, headers: { "Content-Type": "application/vnd.api+json" } },
    );
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers["Content-Type"]).toBe("application/vnd.api+json");
    expect(headers["content-type"]).toBeUndefined();
  });
});

describe("transport call", () => {
  it("issues the call with the descriptor's method and built uri", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "DELETE", uri: "/things/{id}", bindings: [binding("id", { kind: "label" })] }),
      { id: "42" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].init.method).toBe("DELETE");
    expect(calls[0].url).toBe("https://api.test/things/42");
  });
});

describe("response classification", () => {
  it("maps a declared success status to a success outcome", async () => {
    const { fetchImpl } = recorder(200, '{"id":"x"}');
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome).toEqual({ outcome: "success", status: 200, body: '{"id":"x"}' });
  });

  it("treats any 2xx as success even when its code was not the declared one", async () => {
    // The descriptor declares 201, the server answers 200: still a success.
    const { fetchImpl } = recorder(200, '{"ok":true}');
    const outcome = await execute(
      descriptor({ success: [[201, null]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome).toEqual({ outcome: "success", status: 200, body: '{"ok":true}' });
  });

  it("maps a non-success status to an error outcome for the SDK to discriminate", async () => {
    const { fetchImpl } = recorder(404, '{"code":"not_found"}');
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome).toEqual({ outcome: "error", status: 404, body: '{"code":"not_found"}' });
  });

  it("reports a network failure as a transport outcome", async () => {
    const boom = new Error("network down");
    const fetchImpl = (async () => {
      throw boom;
    }) as unknown as typeof fetch;
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome).toEqual({ outcome: "transport", cause: boom });
  });

  it("reports a body-read failure as a transport outcome", async () => {
    const boom = new Error("stream aborted");
    const fetchImpl = (async () =>
      ({
        status: 200,
        text: async () => {
          throw boom;
        },
        headers: new Headers(),
      }) as unknown as Response) as unknown as typeof fetch;
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome).toEqual({ outcome: "transport", cause: boom });
  });
});

describe("response bindings", () => {
  it("folds a response header into the decoded body under its member name", async () => {
    const { fetchImpl } = recorder(200, '{"id":"x"}', { "X-Request-Id": "req-1" });
    const outcome = await execute(
      descriptor({ response_bindings: [["requestId", { kind: "header", name: "X-Request-Id" }]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome).toEqual({
      outcome: "success",
      status: 200,
      body: JSON.stringify({ id: "x", requestId: "req-1" }),
    });
  });

  it("folds the response status code into the decoded body", async () => {
    const { fetchImpl } = recorder(200, '{"id":"x"}');
    const outcome = await execute(
      descriptor({ response_bindings: [["httpStatus", { kind: "statusCode" }]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome).toEqual({
      outcome: "success",
      status: 200,
      body: JSON.stringify({ id: "x", httpStatus: 200 }),
    });
  });
});

describe("verbatim execution", () => {
  it("applies no binding defaults of its own: an explicit descriptor produces exactly that request", async () => {
    const { calls, fetchImpl } = recorder();
    // Every member is explicitly bound; the runtime must not reclassify any.
    await execute(
      descriptor({
        http_method: "PUT",
        uri: "/x/{id}",
        bindings: [
          binding("id", { kind: "label" }),
          binding("q", { kind: "query", name: "q" }),
          binding("h", { kind: "header", name: "H" }),
          binding("field", { kind: "body" }),
        ],
      }),
      { id: "1", q: "2", h: "3", field: "4" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/x/1?q=2");
    expect((calls[0].init.headers as Record<string, string>)["H"]).toBe("3");
    expect(calls[0].init.body).toBe(JSON.stringify({ field: "4" }));
  });
});

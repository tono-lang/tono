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
  it("drops a non-JSON body and lets the bound fields stand on their own", async () => {
    const { fetchImpl } = recorder(200, "not json", { "X-Request-Id": "req-1" });
    const outcome = await execute(
      descriptor({ response_bindings: [["requestId", { kind: "header", name: "X-Request-Id" }]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome).toEqual({
      outcome: "success",
      status: 200,
      body: JSON.stringify({ requestId: "req-1" }),
    });
  });

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

describe("edge cases pinned by the mutation gate", () => {
  it("normalizes a null input to an empty record instead of dereferencing it", async () => {
    // A null input with a body binding must not be indexed into; it yields no body.
    const { calls, fetchImpl } = recorder();
    const outcome = await execute(
      descriptor({ bindings: [binding("a", { kind: "body" })] }),
      null,
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome.outcome).toBe("success");
    expect(calls[0].init.body).toBeUndefined();
  });

  it("normalizes a non-object input to an empty record", async () => {
    // A string input has no members: a query binding named after a string
    // property ("length") must not leak the string's own length.
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        http_method: "GET",
        uri: "/things",
        bindings: [binding("length", { kind: "query", name: "length" })],
      }),
      "hello",
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things");
  });

  it("leaves a non-label binding out of path substitution", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        http_method: "GET",
        uri: "/things/{id}",
        bindings: [binding("id", { kind: "label" }), binding("q", { kind: "query", name: "q" })],
      }),
      { id: "7", q: "x" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/7?q=x");
  });

  it("substitutes a present label rather than emptying it", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things/{id}", bindings: [binding("id", { kind: "label" })] }),
      { id: "present" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/present");
  });

  it("omits an absent query member and keeps a present one", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        http_method: "GET",
        uri: "/things",
        bindings: [binding("q", { kind: "query", name: "q" }), binding("r", { kind: "query", name: "r" })],
      }),
      { q: "kept" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things?q=kept");
  });

  it("omits a null header and keeps a present one", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        bindings: [
          binding("keep", { kind: "header", name: "X-Keep" }),
          binding("drop", { kind: "header", name: "X-Drop" }),
        ],
      }),
      { keep: "yes", drop: null },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers["X-Keep"]).toBe("yes");
    expect(headers["X-Drop"]).toBeUndefined();
  });

  it("drops an absent body member from the assembled object", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" }), binding("b", { kind: "body" })] }),
      { a: 1 },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].init.body).toBe(JSON.stringify({ a: 1 }));
  });

  it("sends no body and no content-type when every body member is absent", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" })] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].init.body).toBeUndefined();
    expect((calls[0].init.headers as Record<string, string>)["content-type"]).toBeUndefined();
  });

  it("adds the default content-type when a body is present and none was set", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" })] }),
      { a: 1 },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect((calls[0].init.headers as Record<string, string>)["content-type"]).toBe("application/json");
  });

  it("suppresses the default content-type when the caller set it in exact lower case", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" })] }),
      { a: 1 },
      { baseUrl: "https://api.test", fetch: fetchImpl, headers: { "content-type": "text/plain" } },
    );
    expect((calls[0].init.headers as Record<string, string>)["content-type"]).toBe("text/plain");
  });

  it("classifies the 2xx boundary: 299 succeeds, 300 errors", async () => {
    const ok = recorder(299, "{}");
    expect(
      (await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: ok.fetchImpl })).outcome,
    ).toBe("success");
    const bad = recorder(300, "{}");
    expect(
      (await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: bad.fetchImpl })).outcome,
    ).toBe("error");
  });

  it("honors a declared success code outside the 2xx range", async () => {
    const { fetchImpl } = recorder(302, "{}");
    const outcome = await execute(
      descriptor({ success: [[302, null]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome.outcome).toBe("success");
  });

  it("does not treat an undeclared non-2xx code as success", async () => {
    const { fetchImpl } = recorder(418, "{}");
    const outcome = await execute(
      descriptor({ success: [[302, null]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome.outcome).toBe("error");
  });

  it("returns the body verbatim when there are no response bindings", async () => {
    // A spaced body proves the text is passed through, not re-serialized: the
    // early return must fire when there is nothing to fold in.
    const { fetchImpl } = recorder(200, '{"id": "x"}');
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome).toEqual({ outcome: "success", status: 200, body: '{"id": "x"}' });
  });

  it("only substitutes label bindings into the path, not query bindings", async () => {
    // A query binding whose name matches a path placeholder must be left as a
    // literal: only labels feed path substitution.
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things/{q}", bindings: [binding("q", { kind: "query", name: "q" })] }),
      { q: "v" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/{q}?q=v");
  });

  it("substitutes a null label as empty rather than the literal null", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things/{id}", bindings: [binding("id", { kind: "label" })] }),
      { id: null },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(calls[0].url).toBe("https://api.test/things/");
  });

  it("only reads header bindings into headers, not query bindings", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ http_method: "GET", uri: "/things", bindings: [binding("q", { kind: "query", name: "q" })] }),
      { q: "v" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect((calls[0].init.headers as Record<string, string>)["q"]).toBeUndefined();
  });

  it("omits an absent header member rather than sending the literal undefined", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("trace", { kind: "header", name: "X-Trace" })] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect((calls[0].init.headers as Record<string, string>)["X-Trace"]).toBeUndefined();
  });

  it("adds the default content-type even when another header is already present", async () => {
    // hasHeader must match the content-type name specifically, not just report
    // that some header exists.
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ bindings: [binding("a", { kind: "body" }), binding("trace", { kind: "header", name: "X-Trace" })] }),
      { a: 1, trace: "t" },
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers["X-Trace"]).toBe("t");
    expect(headers["content-type"]).toBe("application/json");
  });

  it("requires a 2xx floor: a sub-200 status is not a success on its own", async () => {
    // The Response constructor forbids a sub-200 status, so a bare stub stands in
    // to exercise the lower bound of the 2xx check.
    const fetchImpl = (async () =>
      ({ status: 150, text: async () => "{}", headers: new Headers() }) as unknown as Response) as unknown as typeof fetch;
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome.outcome).toBe("error");
  });

  it("accepts a status matching any one declared success code, not all of them", async () => {
    const { fetchImpl } = recorder(302, "{}");
    const outcome = await execute(
      descriptor({ success: [[301, null], [302, null]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome.outcome).toBe("success");
  });

  it("stands a response-bound field on its own when the body is empty", async () => {
    const { fetchImpl } = recorder(200, "", { "X-Id": "r1" });
    const outcome = await execute(
      descriptor({ response_bindings: [["id", { kind: "header", name: "X-Id" }]] }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome).toEqual({ outcome: "success", status: 200, body: JSON.stringify({ id: "r1" }) });
  });
});

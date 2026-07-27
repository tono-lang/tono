// The entry-scoped descriptor positions: the endpoint field reference, the
// declared request headers (key template -> value expression), and the
// {.field} path placeholders. All of them resolve against the resolved client
// values in ClientOptions.values; the runtime never learns where those values
// came from.

import { describe, expect, it } from "vitest";

import { execute } from "../src/execute";
import type { CanonicalRequest, WireDescriptor } from "../src/descriptor";

function recorder(status = 200, body = "{}") {
  const calls: { url: string; init: RequestInit }[] = [];
  const fetchImpl = (async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    return new Response(body, { status });
  }) as unknown as typeof fetch;
  return { calls, fetchImpl };
}

function headerOf(init: RequestInit, name: string): string | undefined {
  return (init.headers as Record<string, string>)[name];
}

function descriptor(over: Partial<WireDescriptor> = {}): WireDescriptor {
  return {
    http_method: "GET",
    uri: "/things",
    bindings: [],
    response_bindings: [],
    success: [[200, null]],
    errors: [],
    ...over,
  };
}

describe("the endpoint field reference", () => {
  it("overrides baseUrl with the string under the dotted path", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(descriptor({ endpoint: ["conf", "url"] }), {}, {
      baseUrl: "https://fallback.test",
      fetch: fetchImpl,
      values: { "conf.url": "https://acme.test" },
    });
    expect(calls[0].url).toBe("https://acme.test/things");
  });

  it("falls back to baseUrl when the value is absent, empty, or not a string", async () => {
    for (const values of [{}, { endpoint: "" }, { endpoint: 7 }]) {
      const { calls, fetchImpl } = recorder();
      await execute(descriptor({ endpoint: ["endpoint"] }), {}, {
        baseUrl: "https://fallback.test",
        fetch: fetchImpl,
        values,
      });
      expect(calls[0].url).toBe("https://fallback.test/things");
    }
  });

  it("ignores an empty endpoint declaration", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(descriptor({ endpoint: [] }), {}, { baseUrl: "https://b.test", fetch: fetchImpl });
    expect(calls[0].url).toBe("https://b.test/things");
  });
});

describe("the {.field} path placeholders", () => {
  it("substitutes and escapes the resolved value", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({ uri: "/v/{.tenant}/things/{id}", bindings: [["id", { kind: "label" }]] }),
      { id: "7" },
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { tenant: "acme corp" } },
    );
    expect(calls[0].url).toBe("https://b.test/v/acme%20corp/things/7");
  });

  it("substitutes an absent or null field as empty and formats numbers plainly", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(descriptor({ uri: "/a/{.gone}/b/{.n}/{.missing}" }), {}, {
      baseUrl: "https://b.test",
      fetch: fetchImpl,
      values: { gone: null, n: 2 },
    });
    expect(calls[0].url).toBe("https://b.test/a//b/2/");
  });

  it("substitutes a placeholder at the very start and keeps a brace in the tail", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(descriptor({ uri: "{.prefix}/x}y/{.n}" }), {}, {
      baseUrl: "https://b.test",
      fetch: fetchImpl,
      values: { prefix: "/p", n: 1 },
    });
    expect(calls[0].url).toBe("https://b.test%2Fp/x}y/1");
  });

  it("leaves an unterminated placeholder and label-style braces verbatim", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(descriptor({ uri: "/p/{id}/{.open" }), {}, {
      baseUrl: "https://b.test",
      fetch: fetchImpl,
      values: { open: "x" },
    });
    expect(calls[0].url).toBe("https://b.test/p/{id}/{.open");
  });
});

describe("the declared request headers", () => {
  it("resolves literal, field, and template values against the client values", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        request_headers: [
          [[{ lit: "X-Static" }], { lit: "s" }],
          [[{ lit: "X-Client" }], { field: ["client_name"] }],
          [[{ lit: "Authorization" }], { template: [{ lit: "Bearer " }, { field: ["token"] }] }],
          [[{ lit: "X-Num" }], { lit: 3 }],
        ],
      }),
      {},
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { client_name: "demo", token: "t0" } },
    );
    const init = calls[0].init;
    expect(headerOf(init, "X-Static")).toBe("s");
    expect(headerOf(init, "X-Client")).toBe("demo");
    expect(headerOf(init, "Authorization")).toBe("Bearer t0");
    expect(headerOf(init, "X-Num")).toBe("3");
  });

  it("renders a templated key from field and input parts", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        bindings: [["id", { kind: "body" }]],
        request_headers: [[[{ lit: "X-" }, { field: ["kind"] }, { input: "id" }], { lit: "v" }]],
      }),
      { id: "9" },
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { kind: "K" } },
    );
    expect(headerOf(calls[0].init, "X-K9")).toBe("v");
  });

  it("omits a header whose key or value cannot resolve, whole", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        request_headers: [
          [[{ lit: "X-NoValue" }], { field: ["missing"] }],
          [[{ field: ["missing"] }], { lit: "v" }],
          [[{ lit: "X-NullLit" }], { lit: null }],
          [[], { lit: "empty key" }],
          [[{ lit: "X-NoInput" }, { input: "gone" }], { lit: "v" }],
        ],
      }),
      {},
      { baseUrl: "https://b.test", fetch: fetchImpl, values: {} },
    );
    expect(Object.keys(calls[0].init.headers as Record<string, string>)).toEqual([]);
  });

  it("omits a header whose value expression is an absent literal or a null field", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        request_headers: [
          [[{ lit: "X-Undef" }], { lit: undefined }],
          [[{ lit: "X-NullField" }], { field: ["n"] }],
        ],
      }),
      {},
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { n: null } },
    );
    expect(Object.keys(calls[0].init.headers as Record<string, string>)).toEqual([]);
  });

  it("renders scalars the wire way: booleans, dotted field paths, structured literals", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        request_headers: [
          [[{ lit: "X-Flag" }], { field: ["flag"] }],
          [[{ lit: "X-Deep" }], { field: ["a", "b"] }],
          [[{ lit: "X-List" }], { lit: [1, 2] }],
          [[{ lit: "X-Broken" }], { field: ["big"] }],
        ],
      }),
      {},
      // A bigint is not JSON-serializable: formatScalar falls back to "".
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { flag: true, "a.b": "x", big: 10n } },
    );
    const init = calls[0].init;
    expect(headerOf(init, "X-Flag")).toBe("true");
    expect(headerOf(init, "X-Deep")).toBe("x");
    expect(headerOf(init, "X-List")).toBe("[1,2]");
    expect(headerOf(init, "X-Broken")).toBe("");
  });

  it("resolves dotted field paths inside a key template and omits a null part", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        request_headers: [
          [[{ lit: "X-" }, { field: ["a", "b"] }], { lit: "v" }],
          [[{ lit: "X-T" }], { template: [{ field: ["n"] }] }],
          [[{ lit: "X-Fn" }], { field: ["fn"] }],
        ],
      }),
      {},
      {
        baseUrl: "https://b.test",
        fetch: fetchImpl,
        values: { "a.b": "K", n: null, fn: () => {} },
      },
    );
    const init = calls[0].init;
    expect(headerOf(init, "X-K")).toBe("v");
    // A null template part omits the header whole; an unserializable value
    // renders empty rather than failing.
    expect("X-T" in (init.headers as Record<string, string>)).toBe(false);
    expect(headerOf(init, "X-Fn")).toBe("");
  });

  it("is layered under the caller's headers and the input's header bindings", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        bindings: [["trace", { kind: "header", name: "X-Trace" }]],
        request_headers: [
          [[{ lit: "Authorization" }], { lit: "declared" }],
          [[{ lit: "X-Trace" }], { lit: "declared" }],
          [[{ lit: "X-Kept" }], { lit: "declared" }],
        ],
      }),
      { trace: "t1" },
      {
        baseUrl: "https://b.test",
        fetch: fetchImpl,
        headers: { Authorization: "Bearer caller" },
      },
    );
    const init = calls[0].init;
    // The caller's (bespoke) header wins over the declared one; the per-call
    // binding is the most specific and wins over both.
    expect(headerOf(init, "Authorization")).toBe("Bearer caller");
    expect(headerOf(init, "X-Trace")).toBe("t1");
    expect(headerOf(init, "X-Kept")).toBe("declared");
  });

  it("is applied before the before_request hook runs", async () => {
    const { fetchImpl } = recorder();
    let saw: CanonicalRequest | undefined;
    await execute(
      descriptor({ request_headers: [[[{ lit: "Authorization" }], { template: [{ lit: "Bearer " }, { field: ["token"] }] }]] }),
      {},
      { baseUrl: "https://b.test", fetch: fetchImpl, values: { token: "t0" } },
      {
        before_request: (req) => {
          saw = req;
          return req;
        },
      },
    );
    expect(saw?.headers["Authorization"]).toBe("Bearer t0");
  });
});

describe("header layering across casings", () => {
  it("lets a bespoke or per-call header override a declared one under any casing", async () => {
    const { calls, fetchImpl } = recorder();
    await execute(
      descriptor({
        bindings: [["trace", { kind: "header", name: "x-trace" }]],
        request_headers: [
          [[{ lit: "Authorization" }], { lit: "declared" }],
          [[{ lit: "X-Trace" }], { lit: "declared" }],
        ],
      }),
      { trace: "t1" },
      { baseUrl: "https://b.test", fetch: fetchImpl, headers: { authorization: "Bearer bespoke" } },
    );
    const headers = calls[0].init.headers as Record<string, string>;
    expect(headers["authorization"]).toBe("Bearer bespoke");
    expect("Authorization" in headers).toBe(false);
    expect(headers["x-trace"]).toBe("t1");
    expect("X-Trace" in headers).toBe(false);
    expect(Object.keys(headers).length).toBe(2);
  });
});

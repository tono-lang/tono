import { describe, expect, it } from "vitest";

import { execute } from "../src/execute";
import type {
  CanonicalRequest,
  CanonicalResponse,
  Hooks,
  WireDescriptor,
} from "../src/descriptor";

// A recording transport: captures the request the runtime built (after
// before_request ran) and returns a canned response.
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

// The boundary wrapper the generated client emits, mirrored here so the runtime
// test exercises the pass-through-vs-wrap decision the SDK relies on.
class ContractError extends Error {
  constructor(
    readonly contractName: string,
    readonly cause: unknown,
  ) {
    super(`contract hook '${contractName}' failed`);
  }
}
class DeclaredError extends Error {}

function wrapBeforeRequest(fn: (r: CanonicalRequest) => CanonicalRequest): Hooks["before_request"] {
  return (req) => {
    try {
      return fn(req);
    } catch (e) {
      if (e instanceof DeclaredError) throw e;
      throw new ContractError("before_request", e);
    }
  };
}

describe("lifecycle hooks", () => {
  it("runs before_request before the transport call and after_response after it", async () => {
    const order: string[] = [];
    const { calls, fetchImpl } = recorder(200, "{}");
    const hooks: Hooks = {
      before_request: (req) => {
        order.push("before");
        return { ...req, headers: { ...req.headers, authorization: "Bearer t" } };
      },
      after_response: (res) => {
        order.push("after");
        return res;
      },
    };
    const transport = (async (url: string | URL | Request, init?: RequestInit) => {
      order.push("fetch");
      return fetchImpl(url as string, init as RequestInit);
    }) as unknown as typeof fetch;

    await execute(
      descriptor({ http_method: "GET", uri: "/things" }),
      {},
      { baseUrl: "https://api.test", fetch: transport },
      hooks,
    );

    expect(order).toEqual(["before", "fetch", "after"]);
    // before_request's mutation reached the wire.
    const sent = calls[0].init.headers as Record<string, string>;
    expect(sent.authorization).toBe("Bearer t");
  });

  it("lets before_request rewrite the url and body", async () => {
    const { calls, fetchImpl } = recorder();
    const hooks: Hooks = {
      before_request: (req) => ({ ...req, url: req.url + "?signed=1", body: "signed" }),
    };
    await execute(
      descriptor(),
      { a: 1 },
      { baseUrl: "https://api.test", fetch: fetchImpl },
      hooks,
    );
    expect(calls[0].url).toBe("https://api.test/things?signed=1");
    expect(calls[0].init.body).toBe("signed");
  });

  it("lets after_response rewrite the body before classification", async () => {
    const { fetchImpl } = recorder(200, '{"raw":true}');
    const hooks: Hooks = {
      after_response: (res: CanonicalResponse) => ({ ...res, body: '{"patched":true}' }),
    };
    const outcome = await execute(
      descriptor(),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
      hooks,
    );
    expect(outcome).toEqual({ outcome: "success", status: 200, body: '{"patched":true}' });
  });

  it("propagates a hook throw raw; the runtime never turns it into a transport outcome", async () => {
    const { fetchImpl } = recorder();
    const hooks: Hooks = {
      before_request: wrapBeforeRequest(() => {
        throw new Error("boom");
      }),
    };
    // A non-declared throw surfaces as ContractError with the hook name and cause.
    await expect(
      execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl }, hooks),
    ).rejects.toMatchObject({ contractName: "before_request" });
  });

  it("passes a declared error through the wrapper untouched", async () => {
    const { fetchImpl } = recorder();
    const declared = new DeclaredError("nope");
    const hooks: Hooks = {
      before_request: wrapBeforeRequest(() => {
        throw declared;
      }),
    };
    await expect(
      execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl }, hooks),
    ).rejects.toBe(declared);
  });

  it("is a no-op when no hooks are supplied (backward compatible)", async () => {
    const { calls, fetchImpl } = recorder(200, "{}");
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", fetch: fetchImpl });
    expect(outcome.outcome).toBe("success");
    expect(calls).toHaveLength(1);
  });
});

import { afterEach, describe, expect, it, vi } from "vitest";

import type {
  CanonicalResponse,
  CanonicalTransport,
  Outcome,
  WireDescriptor,
} from "../src/descriptor";
import { assertExclusiveTransport, execute } from "../src/execute";
import {
  backoffDelayMs,
  isRetryable,
  resolveMaxRetries,
  resolveTimeoutMs,
} from "../src/retry";

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

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

// Deterministic seams: jitter pinned to 0.5 and a recording, instant sleep.
function seams() {
  const delays: number[] = [];
  return {
    delays,
    seams: {
      sleep: (ms: number) => {
        delays.push(ms);
        return Promise.resolve();
      },
      random: () => 0.5,
    },
  };
}

// A canonical transport answering from a script, one step per attempt.
type Step =
  | { kind: "response"; status: number; body: string }
  | { kind: "transport_failure" }
  | { kind: "hang" };

function scripted(steps: Step[]) {
  let attempts = 0;
  const transport: CanonicalTransport = (_req, signal) => {
    if (attempts >= steps.length) {
      return Promise.reject(new Error(`transport called ${attempts + 1} times for a ${steps.length}-step script`));
    }
    const step = steps[attempts];
    attempts += 1;
    if (step.kind === "response") {
      return Promise.resolve({ status: step.status, headers: {}, body: step.body });
    }
    if (step.kind === "transport_failure") {
      return Promise.reject(new Error("scripted transport failure"));
    }
    // Honor the abort signal, as the canonical transport contract asks; bail
    // out fast if a mutant keeps the timeout from firing.
    return new Promise<CanonicalResponse>((_, reject) => {
      const bail = setTimeout(() => reject(new Error("hang step: the per-attempt timeout never fired")), 2000);
      signal?.addEventListener("abort", () => {
        clearTimeout(bail);
        reject(signal.reason instanceof Error ? signal.reason : new Error("aborted"));
      });
    });
  };
  return { transport, attempts: () => attempts };
}

describe("value resolution", () => {
  it("resolves the retry maximum from a literal, flooring and disabling below one", () => {
    expect(resolveMaxRetries(undefined, undefined)).toBe(0);
    expect(resolveMaxRetries(null, undefined)).toBe(0);
    expect(resolveMaxRetries({ max: { lit: 2 } }, undefined)).toBe(2);
    expect(resolveMaxRetries({ max: { lit: 2.9 } }, undefined)).toBe(2);
    expect(resolveMaxRetries({ max: { lit: 0.5 } }, undefined)).toBe(0);
    expect(resolveMaxRetries({ max: { lit: 0 } }, undefined)).toBe(0);
    expect(resolveMaxRetries({ max: { lit: -3 } }, undefined)).toBe(0);
    // A malformed descriptor with a non-numeric literal disables, not crashes.
    expect(resolveMaxRetries({ max: { lit: "3" as unknown as number } }, undefined)).toBe(0);
  });

  it("resolves the retry maximum through a ref into the client values", () => {
    expect(resolveMaxRetries({ max: { ref: "m" } }, { m: 3 })).toBe(3);
    expect(resolveMaxRetries({ max: { ref: "m" } }, { m: "3" })).toBe(0);
    expect(resolveMaxRetries({ max: { ref: "m" } }, { m: Infinity })).toBe(0);
    expect(resolveMaxRetries({ max: { ref: "absent" } }, {})).toBe(0);
    expect(resolveMaxRetries({ max: { ref: "m" } }, undefined)).toBe(0);
  });

  it("resolves the timeout, treating absence as none", () => {
    expect(resolveTimeoutMs(undefined, undefined)).toBe(0);
    expect(resolveTimeoutMs(null, undefined)).toBe(0);
    expect(resolveTimeoutMs({ lit: 1500 }, undefined)).toBe(1500);
    expect(resolveTimeoutMs({ ref: "t" }, { t: 250 })).toBe(250);
    expect(resolveTimeoutMs({ ref: "absent" }, {})).toBe(0);
  });
});

describe("backoff", () => {
  it("grows exponentially from the base and caps, scaled by the jitter", () => {
    expect(backoffDelayMs(0, 0.5)).toBe(50);
    expect(backoffDelayMs(1, 0.5)).toBe(100);
    expect(backoffDelayMs(2, 0.5)).toBe(200);
    expect(backoffDelayMs(3, 0.5)).toBe(400);
    expect(backoffDelayMs(4, 0.5)).toBe(800);
    // 100 * 2^5 = 3200 caps at 2000.
    expect(backoffDelayMs(5, 0.5)).toBe(1000);
    expect(backoffDelayMs(9, 1)).toBe(2000);
    expect(backoffDelayMs(3, 0)).toBe(0);
  });
});

describe("retryable classification", () => {
  // Stands in for a generated decode<Op>Error + retryable() pair: the first
  // status match with an agreeing code decides (a declared code must equal
  // the body's "code" field; a null code matches any body).
  function classifyLikeDiscriminator(
    declared: ReadonlyArray<readonly [number, string | null, boolean]>,
  ): (status: number, body: string) => boolean {
    return (status, body) => {
      let code: unknown;
      try {
        const parsed: unknown = JSON.parse(body);
        code = parsed !== null && typeof parsed === "object" ? (parsed as Record<string, unknown>)["code"] : undefined;
      } catch {
        code = undefined;
      }
      for (const [declaredStatus, declaredCode, retryable] of declared) {
        if (declaredStatus !== status) continue;
        if (declaredCode === null || code === declaredCode) return retryable;
      }
      return false;
    };
  }

  const retryable = classifyLikeDiscriminator([
    [429, "overloaded", true],
    [429, "quota", false],
    [503, null, true],
    [404, null, false],
  ]);
  const error = (status: number, body: string): Outcome => ({ outcome: "error", status, body });

  it("always retries a transport failure and never a success", () => {
    expect(isRetryable({ outcome: "transport", cause: new Error("x") }, retryable)).toBe(true);
    expect(isRetryable({ outcome: "success", status: 200, body: "{}" }, retryable)).toBe(false);
    // Even when a success status collides with a declared retryable error
    // (a declared non-2xx success), the outcome kind decides.
    expect(
      isRetryable({ outcome: "success", status: 429, body: '{"code":"overloaded"}' }, retryable),
    ).toBe(false);
  });

  it("discriminates declared errors by status and code", () => {
    expect(isRetryable(error(429, '{"code":"overloaded"}'), retryable)).toBe(true);
    expect(isRetryable(error(429, '{"code":"quota"}'), retryable)).toBe(false);
    expect(isRetryable(error(429, '{"code":"other"}'), retryable)).toBe(false);
    expect(isRetryable(error(429, "not json"), retryable)).toBe(false);
    expect(isRetryable(error(429, "null"), retryable)).toBe(false);
    expect(isRetryable(error(429, '{"code":5}'), retryable)).toBe(false);
    expect(isRetryable(error(503, "not json"), retryable)).toBe(true);
    expect(isRetryable(error(404, "{}"), retryable)).toBe(false);
    expect(isRetryable(error(500, "{}"), retryable)).toBe(false);
  });

  it("treats an absent predicate (no declared errors) as never retryable", () => {
    expect(isRetryable(error(503, "{}"), undefined)).toBe(false);
  });
});

describe("transport slot exclusivity", () => {
  it("rejects both slots set, at assertion and at execute", async () => {
    const { transport } = scripted([{ kind: "response", status: 200, body: "{}" }]);
    const options = { baseUrl: "https://api.test", fetch, transport };
    expect(() => assertExclusiveTransport(options)).toThrow(/mutually exclusive/);
    await expect(execute(descriptor(), {}, options)).rejects.toThrow(/mutually exclusive/);
  });

  it("accepts each slot alone", () => {
    const { transport } = scripted([]);
    expect(() => assertExclusiveTransport({ baseUrl: "b", transport })).not.toThrow();
    expect(() => assertExclusiveTransport({ baseUrl: "b", fetch })).not.toThrow();
    expect(() => assertExclusiveTransport({ baseUrl: "b" })).not.toThrow();
  });
});

describe("canonical transport slot", () => {
  it("receives the built request and its response is classified as usual", async () => {
    let seen: unknown;
    const transport: CanonicalTransport = (req) => {
      seen = req;
      return Promise.resolve({ status: 200, headers: { "x-request-id": "r1" }, body: '{"id":"x"}' });
    };
    const outcome = await execute(
      descriptor({
        http_method: "PUT",
        uri: "/x/{id}",
        bindings: [
          ["id", { kind: "label" }],
          ["field", { kind: "body" }],
        ],
        response_bindings: [["requestId", { kind: "header", name: "X-Request-Id" }]],
      }),
      { id: "1", field: "4" },
      { baseUrl: "https://api.test", transport, headers: { Authorization: "Bearer t" } },
    );
    expect(seen).toEqual({
      method: "PUT",
      url: "https://api.test/x/1",
      headers: { Authorization: "Bearer t", "content-type": "application/json" },
      body: '{"field":"4"}',
    });
    expect(outcome).toEqual({
      outcome: "success",
      status: 200,
      body: JSON.stringify({ id: "x", requestId: "r1" }),
    });
  });

  it("reports a canonical transport rejection as a transport outcome", async () => {
    const boom = new Error("down");
    const transport: CanonicalTransport = () => Promise.reject(boom);
    const outcome = await execute(descriptor(), {}, { baseUrl: "https://api.test", transport });
    expect(outcome).toEqual({ outcome: "transport", cause: boom });
  });

  it("passes no signal when the descriptor declares no timeout", async () => {
    let sawSignal: AbortSignal | undefined = undefined;
    const transport: CanonicalTransport = (_req, signal) => {
      sawSignal = signal;
      return Promise.resolve({ status: 200, headers: {}, body: "{}" });
    };
    await execute(descriptor(), {}, { baseUrl: "https://api.test", transport });
    expect(sawSignal).toBeUndefined();
  });
});

describe("retry loop", () => {
  it("honors the maximum and the backoff sequence", async () => {
    const { transport, attempts } = scripted([
      { kind: "transport_failure" },
      { kind: "transport_failure" },
      { kind: "transport_failure" },
      { kind: "transport_failure" },
    ]);
    const { delays, seams: s } = seams();
    const outcome = await execute(
      descriptor({ retry: { max: { lit: 3 } } }),
      {},
      { baseUrl: "https://api.test", transport },
      undefined,
      undefined,
      s,
    );
    expect(outcome.outcome).toBe("transport");
    expect(attempts()).toBe(4);
    expect(delays).toEqual([50, 100, 200]);
  });

  it("stops at the first non-retryable outcome", async () => {
    const { transport, attempts } = scripted([
      { kind: "response", status: 503, body: "{}" },
      { kind: "response", status: 404, body: "{}" },
    ]);
    const { delays, seams: s } = seams();
    const outcome = await execute(
      descriptor({ retry: { max: { lit: 5 } } }),
      {},
      { baseUrl: "https://api.test", transport },
      undefined,
      (status) => status === 503,
      s,
    );
    expect(outcome).toEqual({ outcome: "error", status: 404, body: "{}" });
    expect(attempts()).toBe(2);
    expect(delays).toEqual([50]);
  });

  it("retries on the native slot too", async () => {
    let calls = 0;
    const fetchImpl = (async () => {
      calls += 1;
      if (calls === 1) throw new Error("down");
      return new Response("{}", { status: 200 });
    }) as unknown as typeof fetch;
    const { seams: s } = seams();
    const outcome = await execute(
      descriptor({ retry: { max: { lit: 1 } } }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
      undefined,
      undefined,
      s,
    );
    expect(outcome.outcome).toBe("success");
    expect(calls).toBe(2);
  });

  it("really sleeps the backoff delay when no seams are given", async () => {
    vi.useFakeTimers();
    vi.spyOn(Math, "random").mockReturnValue(0.5);
    const { transport, attempts } = scripted([
      { kind: "transport_failure" },
      { kind: "response", status: 200, body: "{}" },
    ]);
    let settled = false;
    const pending = execute(
      descriptor({ retry: { max: { lit: 1 } } }),
      {},
      { baseUrl: "https://api.test", transport },
    ).then((outcome) => {
      settled = true;
      return outcome;
    });
    // The first attempt failed and the retry is waiting out its 50ms backoff
    // (0.5 * 100): one tick short of it, nothing has settled.
    await vi.advanceTimersByTimeAsync(49);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    const outcome = await pending;
    expect(outcome.outcome).toBe("success");
    expect(attempts()).toBe(2);
  });

  it("propagates a before_request hook throw without attempting or retrying", async () => {
    const { transport, attempts } = scripted([{ kind: "response", status: 200, body: "{}" }]);
    const boom = new Error("hook broke");
    await expect(
      execute(
        descriptor({ retry: { max: { lit: 3 } } }),
        {},
        { baseUrl: "https://api.test", transport },
        {
          before_request: () => {
            throw boom;
          },
        },
        undefined,
        seams().seams,
      ),
    ).rejects.toBe(boom);
    expect(attempts()).toBe(0);
  });

  it("propagates an after_response hook throw instead of retrying a retryable response", async () => {
    const { transport, attempts } = scripted([{ kind: "response", status: 503, body: "{}" }]);
    const boom = new Error("hook broke");
    await expect(
      execute(
        descriptor({ retry: { max: { lit: 3 } } }),
        {},
        { baseUrl: "https://api.test", transport },
        {
          after_response: () => {
            throw boom;
          },
        },
        (status) => status === 503,
        seams().seams,
      ),
    ).rejects.toBe(boom);
    expect(attempts()).toBe(1);
  });

  it("classifies for retry on the post-hook response", async () => {
    const { transport, attempts } = scripted([{ kind: "response", status: 503, body: "{}" }]);
    const { delays, seams: s } = seams();
    const outcome = await execute(
      descriptor({ retry: { max: { lit: 3 } } }),
      {},
      { baseUrl: "https://api.test", transport },
      { after_response: (res) => ({ ...res, status: 200 }) },
      (status) => status === 503,
      s,
    );
    expect(outcome.outcome).toBe("success");
    expect(attempts()).toBe(1);
    expect(delays).toEqual([]);
  });
});

describe("per-attempt timeout", () => {
  it("times out a hanging canonical transport, aborting the signal", async () => {
    const { transport, attempts } = scripted([
      { kind: "hang" },
      { kind: "response", status: 200, body: "{}" },
    ]);
    const { delays, seams: s } = seams();
    const outcome = await execute(
      descriptor({ retry: { max: { lit: 1 } }, timeout: { lit: 30 } }),
      {},
      { baseUrl: "https://api.test", transport },
      undefined,
      undefined,
      s,
    );
    expect(outcome.outcome).toBe("success");
    expect(attempts()).toBe(2);
    expect(delays).toEqual([50]);
  });

  it("surfaces a timeout without retry as a transport outcome naming the budget", async () => {
    const { transport } = scripted([{ kind: "hang" }]);
    const outcome = await execute(
      descriptor({ timeout: { lit: 30 } }),
      {},
      { baseUrl: "https://api.test", transport },
    );
    expect(outcome.outcome).toBe("transport");
    expect((outcome as { cause: Error }).cause.message).toBe("attempt timed out after 30ms");
  });

  it("times out a transport that ignores the signal entirely", async () => {
    const transport: CanonicalTransport = () => new Promise<CanonicalResponse>(() => {});
    const outcome = await execute(
      descriptor({ timeout: { lit: 20 } }),
      {},
      { baseUrl: "https://api.test", transport },
    );
    expect(outcome.outcome).toBe("transport");
  });

  it("hands the signal to the native fetch when a timeout is declared", async () => {
    let sawSignal: unknown;
    const fetchImpl = (async (_url: unknown, init?: RequestInit) => {
      sawSignal = init?.signal;
      return new Response("{}", { status: 200 });
    }) as unknown as typeof fetch;
    const outcome = await execute(
      descriptor({ timeout: { lit: 1000 } }),
      {},
      { baseUrl: "https://api.test", fetch: fetchImpl },
    );
    expect(outcome.outcome).toBe("success");
    expect(sawSignal).toBeInstanceOf(AbortSignal);
  });

  it("does not time out an attempt that answers within the budget", async () => {
    const { transport } = scripted([{ kind: "response", status: 200, body: "{}" }]);
    const outcome = await execute(
      descriptor({ timeout: { lit: 1000 } }),
      {},
      { baseUrl: "https://api.test", transport },
    );
    expect(outcome).toEqual({ outcome: "success", status: 200, body: "{}" });
  });

  it("clears the expiry timer once the attempt answers", async () => {
    vi.useFakeTimers();
    const { transport } = scripted([{ kind: "response", status: 200, body: "{}" }]);
    const outcome = await execute(
      descriptor({ timeout: { lit: 1000 } }),
      {},
      { baseUrl: "https://api.test", transport },
    );
    expect(outcome.outcome).toBe("success");
    expect(vi.getTimerCount()).toBe(0);
  });
});

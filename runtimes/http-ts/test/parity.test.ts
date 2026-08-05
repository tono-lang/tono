// The cross-runtime parity suite: the same behavior vectors run against every
// HTTP runtime, so retry/timeout/error handling cannot drift between them.
// Jitter is pinned to 0.5 and backoff sleeps are recorded, matching the other
// runtimes' harnesses.

import { describe, expect, it } from "vitest";

import type {
  CanonicalRequest,
  CanonicalResponse,
  CanonicalTransport,
  WireDescriptor,
} from "../src/descriptor";
import { execute } from "../src/execute";
import vectorsJson from "../../parity/vectors.json";

interface ParityStep {
  kind: "response" | "transport_failure" | "hang";
  status?: number;
  body?: string;
}

interface ParityVector {
  name: string;
  descriptor: WireDescriptor;
  // [status, code|null, retryable] triples: a stand-in for what a generated
  // decode<Op>Error + retryable() pair would classify, without depending on
  // any generated code to exist. Absent means no error response ever retries.
  declared_errors?: ReadonlyArray<readonly [number, string | null, boolean]>;
  values: Record<string, unknown>;
  input: Record<string, unknown>;
  script: ParityStep[];
  expect: {
    outcome: string;
    status?: number;
    body?: string;
    attempts: number;
    delays_ms: number[];
    // Assertions on the first request the runtime built: its full URL and a
    // subset of its headers. Optional; vectors that exercise the entry-scoped
    // descriptor positions (endpoint, request_headers, {.field} paths) use it.
    request?: { url?: string; headers?: Record<string, string> };
  };
}

const file = vectorsJson as unknown as { vectors: ParityVector[] };

// Builds the predicate execute() expects, exactly as a generated client would
// build it from its own decode<Op>Error + retryable() pair: the first status
// match with an agreeing code decides (a declared code must equal the body's
// "code" field; a null code matches any body).
function classifyLikeDiscriminator(
  declared: ReadonlyArray<readonly [number, string | null, boolean]>,
): (status: number, body: string) => boolean {
  return (status, body) => {
    let code: unknown;
    try {
      const parsed: unknown = JSON.parse(body);
      code =
        parsed !== null && typeof parsed === "object"
          ? (parsed as Record<string, unknown>)["code"]
          : undefined;
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

function scripted(script: ParityStep[]) {
  let attempts = 0;
  const requests: CanonicalRequest[] = [];
  const transport: CanonicalTransport = (req, signal) => {
    requests.push(req);
    if (attempts >= script.length) {
      return Promise.reject(
        new Error(`transport called ${attempts + 1} times for a ${script.length}-step script`),
      );
    }
    const step = script[attempts];
    attempts += 1;
    if (step.kind === "response") {
      return Promise.resolve({ status: step.status ?? 0, headers: {}, body: step.body ?? "" });
    }
    if (step.kind === "transport_failure") {
      return Promise.reject(new Error("scripted transport failure"));
    }
    // Honor the abort signal, as the canonical transport contract asks; bail
    // out fast if a mutant keeps the timeout from firing.
    return new Promise<CanonicalResponse>((_, reject) => {
      const bail = setTimeout(
        () => reject(new Error("hang step: the per-attempt timeout never fired")),
        2000,
      );
      signal?.addEventListener("abort", () => {
        clearTimeout(bail);
        reject(signal.reason instanceof Error ? signal.reason : new Error("aborted"));
      });
    });
  };
  return { transport, attempts: () => attempts, requests };
}

describe("parity vectors", () => {
  expect(file.vectors.length).toBeGreaterThan(0);
  for (const vector of file.vectors) {
    it(vector.name, async () => {
      const { transport, attempts, requests } = scripted(vector.script);
      const delays: number[] = [];
      const outcome = await execute(
        vector.descriptor,
        vector.input,
        { baseUrl: "https://api.test", transport, values: vector.values },
        undefined,
        vector.declared_errors ? classifyLikeDiscriminator(vector.declared_errors) : undefined,
        {
          sleep: (ms: number) => {
            delays.push(ms);
            return Promise.resolve();
          },
          random: () => 0.5,
        },
      );
      expect(outcome.outcome).toBe(vector.expect.outcome);
      if (outcome.outcome === "transport") {
        expect(outcome.cause).toBeInstanceOf(Error);
      } else {
        expect(outcome.status).toBe(vector.expect.status);
        expect(outcome.body).toBe(vector.expect.body);
      }
      expect(attempts()).toBe(vector.expect.attempts);
      expect(delays).toEqual(vector.expect.delays_ms);
      if (vector.expect.request) {
        const first = requests[0];
        if (vector.expect.request.url !== undefined) {
          expect(first.url).toBe(vector.expect.request.url);
        }
        for (const [name, value] of Object.entries(vector.expect.request.headers ?? {})) {
          expect(first.headers[name]).toBe(value);
        }
      }
    });
  }
});

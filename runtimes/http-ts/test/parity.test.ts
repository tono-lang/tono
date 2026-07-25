// The cross-runtime parity suite: the same behavior vectors run against every
// HTTP runtime, so retry/timeout/error handling cannot drift between them.
// Jitter is pinned to 0.5 and backoff sleeps are recorded, matching the other
// runtimes' harnesses.

import { describe, expect, it } from "vitest";

import type { CanonicalResponse, CanonicalTransport, WireDescriptor } from "../src/descriptor";
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
  values: Record<string, unknown>;
  input: Record<string, unknown>;
  script: ParityStep[];
  expect: {
    outcome: string;
    status?: number;
    body?: string;
    attempts: number;
    delays_ms: number[];
  };
}

const file = vectorsJson as unknown as { vectors: ParityVector[] };

function scripted(script: ParityStep[]) {
  let attempts = 0;
  const transport: CanonicalTransport = (_req, signal) => {
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
  return { transport, attempts: () => attempts };
}

describe("parity vectors", () => {
  expect(file.vectors.length).toBeGreaterThan(0);
  for (const vector of file.vectors) {
    it(vector.name, async () => {
      const { transport, attempts } = scripted(vector.script);
      const delays: number[] = [];
      const outcome = await execute(
        vector.descriptor,
        vector.input,
        { baseUrl: "https://api.test", transport, values: vector.values },
        undefined,
        {
          sleep: (ms) => {
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
    });
  }
});

// The cross-runtime parity suite, TypeScript side: like its Go and Rust
// siblings, drives the real generated SDK compiled from ../spec.tono. This
// file is not run in place: scripts/run-parity.sh copies it (and
// ../vectors.json) into the SDK's own generated output directory before
// invoking Vitest, so the relative imports below resolve against generated
// code, not against this source tree. It lives here, next to the spec and
// vectors it exercises: the hand-written runtimes/http-ts package it used to
// drive is gone, retired once every target started emitting its own
// transport.
//
// Jitter is pinned to 0.5 and backoff sleeps are recorded through the
// generated client's own timingSeam (./http), the same seam PR #86 added
// specifically so retry timing could be pinned from outside the SDK's own
// tests. The per-attempt @timeout abort still uses a real setTimeout
// internally (see httpSendWithTimeout in ./http) — only the retry backoff
// is mocked — so a "hang" vector costs a few real milliseconds, not the
// seconds a naive backoff would otherwise take.

import { describe, expect, it } from "vitest";

import { Client, ClientConfig } from "./parity/client";
import { APIError, Duration, HttpRequest, HttpResponse, HttpTransport, Thing, TransportError } from "./parity/types";
import { timingSeam } from "./http";
import vectorsJson from "./vectors.json";

type ParityOp = "retrying" | "retrying_with_timeout" | "timeout_only" | "strict_success";

interface ParityStep {
  kind: "response" | "transport_failure" | "hang";
  status?: number;
  body?: string;
}

interface ParityVector {
  name: string;
  op: ParityOp;
  // Real client construction values, not descriptor refs: max_retries feeds
  // ClientConfig.maxRetries, timeout_ms feeds ClientConfig.timeout (as a
  // millisecond duration literal).
  config: { max_retries?: number; timeout_ms?: number };
  // Kept for Go/Rust's still-runtime-level harnesses; the generated SDK
  // decides retryability for real, from spec.tono's own @errorCode/@retryable
  // declarations, so this file never reads it.
  declared_errors?: ReadonlyArray<readonly [number, string | null, boolean]>;
  input: Record<string, unknown>;
  script: ParityStep[];
  expect: {
    outcome: "success" | "error" | "transport";
    status?: number;
    body?: string;
    attempts: number;
    delays_ms: number[];
  };
}

const file = vectorsJson as unknown as { vectors: ParityVector[] };

// Scripts one canned answer per attempt, exactly like the runtime-level
// harnesses: response resolves, transport_failure rejects, hang waits out
// the real per-attempt timeout (or a 2s bail-out, if a mutant breaks it).
function scripted(script: ParityStep[]) {
  let attempts = 0;
  const requests: HttpRequest[] = [];
  const transport: HttpTransport = (req, signal) => {
    requests.push(req);
    if (attempts >= script.length) {
      return Promise.reject(
        new Error(`transport called ${attempts + 1} times for a ${script.length}-step script`),
      );
    }
    const step = script[attempts];
    attempts += 1;
    if (step.kind === "response") {
      const response: HttpResponse = { status: step.status ?? 0, headers: {}, body: step.body ?? "" };
      return Promise.resolve(response);
    }
    if (step.kind === "transport_failure") {
      return Promise.reject(new Error("scripted transport failure"));
    }
    return new Promise<HttpResponse>((_, reject) => {
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

function callOp(client: Client, op: ParityOp): Promise<Thing> {
  switch (op) {
    case "retrying":
      return client.retrying({});
    case "retrying_with_timeout":
      return client.retryingWithTimeout({});
    case "timeout_only":
      return client.timeoutOnly({});
    case "strict_success":
      return client.strictSuccess({});
  }
}

function configFor(vector: ParityVector): ClientConfig {
  const config: ClientConfig = {};
  if (vector.config.max_retries !== undefined) config.maxRetries = vector.config.max_retries;
  if (vector.config.timeout_ms !== undefined) {
    config.timeout = `${vector.config.timeout_ms}ms` as Duration;
  }
  return config;
}

describe("parity vectors", () => {
  expect(file.vectors.length).toBeGreaterThan(0);
  for (const vector of file.vectors) {
    it(vector.name, async () => {
      const { transport, attempts, requests } = scripted(vector.script);
      const delays: number[] = [];
      const originalSleep = timingSeam.sleep;
      const originalRandom = timingSeam.random;
      // Module-level state, not a per-call seam: restore it even if the
      // assertions below throw, or a later test in this file (or a parallel
      // one, if Vitest ever runs this suite non-serially) inherits it.
      timingSeam.sleep = (ms) => {
        delays.push(ms);
        return Promise.resolve();
      };
      timingSeam.random = () => 0.5;
      try {
        const client = Client.forTest({ transport }, "https://api.test", configFor(vector));
        let outcome: { kind: "success" | "error" | "transport"; status?: number; body?: string };
        try {
          const value = await callOp(client, vector.op);
          outcome = { kind: "success", status: 200, body: JSON.stringify(value) };
        } catch (err) {
          if (err instanceof TransportError) {
            outcome = { kind: "transport" };
          } else if (err instanceof APIError) {
            outcome = { kind: "error", status: err.status, body: err.body };
          } else {
            throw err;
          }
        }
        expect(outcome.kind).toBe(vector.expect.outcome);
        if (outcome.kind !== "transport") {
          expect(outcome.status).toBe(vector.expect.status);
          expect(outcome.body).toBe(vector.expect.body);
        }
        expect(attempts()).toBe(vector.expect.attempts);
        expect(delays).toEqual(vector.expect.delays_ms);
        expect(requests.length).toBe(vector.expect.attempts);
      } finally {
        timingSeam.sleep = originalSleep;
        timingSeam.random = originalRandom;
      }
    });
  }
});

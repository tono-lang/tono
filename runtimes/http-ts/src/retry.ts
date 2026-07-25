// Retry classification and backoff. The policy is fixed in the runtime
// (exponential with full jitter), only the maximum and the per-attempt timeout
// are declared in the descriptor; the constants are part of the cross-runtime
// parity contract and must match the other HTTP runtimes.

import type { Outcome, RetrySpec, ValueSource, WireDescriptor } from "./descriptor";

export const BACKOFF_BASE_MS = 100;
export const BACKOFF_CAP_MS = 2000;

// Delay before retry n: random() * min(cap, base * 2^n).
export function backoffDelayMs(retry: number, random: number): number {
  return random * Math.min(BACKOFF_CAP_MS, BACKOFF_BASE_MS * 2 ** retry);
}

// A ValueSource yields a number at call time: a literal frozen by the
// compiler, or a reference to a resolved client field looked up in
// ClientOptions.values by canonical name. Anything else (absent ref,
// non-numeric value) yields undefined, which disables the feature rather than
// failing the call: the descriptor and the resolved values are produced by
// generated code, and the runtime stays blind to their provenance.
function resolveNumber(
  source: ValueSource,
  values: Readonly<Record<string, unknown>> | undefined,
): number | undefined {
  if ("lit" in source) return typeof source.lit === "number" ? source.lit : undefined;
  const value = values?.[source.ref];
  // Number.isFinite alone rejects every non-number, so no typeof guard is
  // needed; the cast is for the type checker only.
  return Number.isFinite(value) ? (value as number) : undefined;
}

// The maximum number of retries after the first attempt. No retry declaration,
// a non-numeric value, or a value below one all mean zero retries; a
// fractional value floors.
export function resolveMaxRetries(
  spec: RetrySpec | null | undefined,
  values: Readonly<Record<string, unknown>> | undefined,
): number {
  if (!spec) return 0;
  const n = resolveNumber(spec.max, values);
  return n === undefined || n < 1 ? 0 : Math.floor(n);
}

// The per-attempt timeout in milliseconds. Absent or non-numeric means no
// deadline; a non-positive value flows through and is treated as no deadline
// by the single `timeoutMs > 0` check at the point of use.
export function resolveTimeoutMs(
  source: ValueSource | null | undefined,
  values: Readonly<Record<string, unknown>> | undefined,
): number {
  if (!source) return 0;
  const ms = resolveNumber(source, values);
  return ms === undefined ? 0 : ms;
}

// The "code" discriminator field of an error body: the parsed body's "code"
// member, when the body parses at all. A non-JSON body, a null body, and a
// missing member all yield undefined, which no declared code equals (the
// comparison below is strict, so a non-string member never matches either).
function bodyCode(body: string): unknown {
  let parsed: unknown;
  try {
    parsed = JSON.parse(body);
  } catch {
    return undefined;
  }
  // Only null blows up on member access; any other primitive just yields
  // undefined for it.
  if (parsed === null) return undefined;
  return (parsed as Record<string, unknown>)["code"];
}

// Classify one outcome for the retry loop: a transport failure always
// retries; an error status retries only when it matches a declared retryable
// error. Matching walks the declared errors in descriptor order and the first
// status match with an agreeing code decides (a declared code must equal the
// body's "code" field; a null code matches any body).
export function isRetryable(descriptor: WireDescriptor, outcome: Outcome): boolean {
  if (outcome.outcome === "transport") return true;
  if (outcome.outcome !== "error") return false;
  const code = bodyCode(outcome.body);
  for (const [status, , declaredCode, retryable] of descriptor.errors) {
    if (status !== outcome.status) continue;
    if (declaredCode === null || code === declaredCode) {
      return retryable === true;
    }
  }
  return false;
}

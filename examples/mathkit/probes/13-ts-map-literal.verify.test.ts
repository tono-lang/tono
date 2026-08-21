// Runs the generated SDK against the stand-in library for real: the map
// literal the binding passes reaches `new TableCalculator(entries)` as an
// object keyed by name, and compute() answers the entry keyed "answer".
import { expect, test } from "vitest";
import { Client } from "./mathkit";

test("probe 13 (ts map literal)", async () => {
  const client = await Client.create(42);
  const got = await client.value();
  expect(got).toBe(42);
});

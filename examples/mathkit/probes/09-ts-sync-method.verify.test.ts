// Runs the generated SDK against the stand-in library for real: the client
// holds the value `new SeriesCalculator(samples)` returned, and compute()
// answers synchronously (no Promise) inside the async method wrapper.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 09 (ts sync method)", async () => {
  const client = await Client.create([1, 2, 3]);
  const got = await client.value();
  expect(got).toBe(3);
});

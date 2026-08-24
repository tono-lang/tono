// The TypeScript driver of the FFI bench: constructs the generated client
// against the real stand-in package (no stub anywhere) and checks the values
// that come back through both operations. The fallback answers its first
// calculator, the constant one, so combined_value is the base; the series
// answers its last value.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("the generated client runs against the stand-in library", async () => {
  const client = await Client.create(1.5, "2 * 3", 2, [1, 2, 3]);
  expect(await client.combinedValue()).toBe(1.5);
  expect(await client.seriesValue()).toBe(3);
});

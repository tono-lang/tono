// Runs the generated SDK against the stand-in library for real: the client
// holds the value `FormulaCalculator.parse(expr)` returned, a static method
// reached through the imported class, and compute() evaluates the formula.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 11 (ts static method)", async () => {
  const client = await Client.create("6 * 7");
  const got = await client.value();
  expect(got).toBe(42);
});

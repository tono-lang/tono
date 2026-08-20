// Runs the generated SDK against the stand-in library for real: the client
// holds the value `new ConstantCalculator(base)` returned and reads through
// it.
import { expect, test } from "vitest";
import { Client } from "./mathkit";

test("probe 08 (ts new construction)", async () => {
  const client = await Client.create(2.5);
  const got = await client.value();
  expect(got).toBe(2.5);
});

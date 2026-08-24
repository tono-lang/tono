// Runs the generated SDK against the stand-in library for real: the memo is
// instantiated over the generated Reading type, and what recall answers is
// the Reading the constructor remembered.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 14 (ts generated type reference)", async () => {
  const seed = { value: 2.5, label: "base" };
  const client = await Client.create(seed);
  const got = await client.recall();
  expect(got).toEqual(seed);
});

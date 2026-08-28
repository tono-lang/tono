// Runs the generated SDK against the stand-in library for real: the memo is
// instantiated over the generated Reading type from a literal the SDK builds
// out of the constructor's own arguments, and what recall answers is that
// Reading.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 22 (ts generated struct literal)", async () => {
  const client = await Client.create(2.5, "base");
  const got = await client.recall();
  expect(got).toEqual({ value: 2.5, label: "base" });
});

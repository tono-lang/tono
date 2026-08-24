// Runs the generated SDK against the stand-in library for real: the bigint
// seed the client takes crosses the constructor as a number, and the value
// read back proves the conversion happened before construction.
import { expect, test } from "vitest";
import { Client } from "./mathkit";

test("probe 15 (ts argument coercion)", async () => {
  const client = await Client.create(21n);
  const got = await client.value();
  expect(got).toBe(21);
});

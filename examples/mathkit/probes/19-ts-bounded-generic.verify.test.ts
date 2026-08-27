// Runs the generated SDK against the stand-in library for real: the primary
// provider holds nothing, so the composed source falls back to the pinned
// one, and the profile the client was given comes back through Bounded<T>.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 19 (ts bounded generic)", async () => {
  const defaults = { endpoint: "calc.local", retries: 3 };
  const client = await Client.create(defaults);
  const got = await client.read();
  expect(got).toEqual(defaults);
});

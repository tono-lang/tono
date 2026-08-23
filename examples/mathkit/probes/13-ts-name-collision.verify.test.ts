// Runs the generated SDK against the stand-in library for real: the client
// holds the library's Client (not its own generated Client) and reads
// through it.
import { expect, test } from "vitest";
import { Client } from "./mathkit";

test("probe 13 (ts name collision)", async () => {
  const client = await Client.create("calc.local");
  const got = await client.ping();
  expect(got).toBe("pong from calc.local");
});

// Runs the generated SDK against the stand-in library for real: the library
// is handed the generated Profile class and the mappings, constructs the
// instance itself and fills the field, and the client reads it back as the
// struct the .tono declares.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 20 (ts class reference to a generated struct)", async () => {
  const client = await Client.create("profile", {
    endpoint: { from: "calc.local" },
  });
  const got = await client.read();
  expect(got.endpoint).toBe("calc.local");
});

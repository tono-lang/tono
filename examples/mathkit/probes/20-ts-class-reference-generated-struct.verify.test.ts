// Runs the generated SDK against the stand-in library for real: the library
// is handed the generated Profile class and the mappings table (a Map on
// its side, a plain object on the client's), constructs the instance itself
// and fills the field; the client reads the value back as the struct the
// .tono declares, and the table back as the plain object it passed in.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 20 (ts class reference to a generated struct)", async () => {
  const mappings = { endpoint: { from: "calc.local" } };
  const client = await Client.create("profile", mappings);
  const got = await client.read();
  expect(got.endpoint).toBe("calc.local");
  const table = await client.mappings();
  expect(table).toEqual(mappings);
  expect(table).not.toBeInstanceOf(Map);
});

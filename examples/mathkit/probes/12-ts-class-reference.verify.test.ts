// Runs the generated SDK against the stand-in library for real: the client
// holds the value `instantiate(AnswerCalculator)` returned, the library
// constructing the class the binding passed, and compute() answers.
import { expect, test } from "vitest";
import { Client } from "mathkit-sdk/mathkit";

test("probe 12 (ts class reference)", async () => {
  const client = await Client.create();
  const got = await client.value();
  expect(got).toBe(42);
});

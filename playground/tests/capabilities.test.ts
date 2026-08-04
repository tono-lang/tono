import { describe, expect, it } from "vitest";
import { resolveCapabilities } from "../src/capabilities";

describe("resolveCapabilities", () => {
  it("only the full mode may execute code", () => {
    expect(resolveCapabilities("full")).toEqual({ run: true });
    expect(resolveCapabilities("hosted")).toEqual({ run: false });
    expect(resolveCapabilities(undefined)).toEqual({ run: false });
    expect(resolveCapabilities("anything-else")).toEqual({ run: false });
  });
});

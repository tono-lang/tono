import { describe, expect, it } from "vitest";
import { DEFAULT_MODULE, sanitizeModuleName } from "../src/modname";

describe("sanitizeModuleName", () => {
  it("keeps a canonical snake_case name", () => {
    expect(sanitizeModuleName("github_api")).toBe("github_api");
  });

  it("folds arbitrary input to snake_case", () => {
    expect(sanitizeModuleName("My SDK!")).toBe("my_sdk");
    expect(sanitizeModuleName("2fast")).toBe("fast");
  });

  it("falls back to the default when nothing survives", () => {
    expect(sanitizeModuleName("")).toBe(DEFAULT_MODULE);
    expect(sanitizeModuleName("123")).toBe(DEFAULT_MODULE);
  });
});

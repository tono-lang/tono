import { describe, expect, it } from "vitest";
import { formFromJson, formToJson } from "../src/mockform";

describe("mock form round-trip", () => {
  it("parses mocks.json into rows and back", () => {
    const json = `{"env":{"API":"x"},"routes":{"GET /a":{"status":201,"body":{"ok":true}}},"passthrough":true}`;
    const form = formFromJson(json);
    expect(form).not.toBeNull();
    expect(form!.env).toEqual([{ key: "API", value: "x" }]);
    expect(form!.routes[0]).toMatchObject({ method: "GET", path: "/a", status: "201" });
    expect(form!.passthrough).toBe(true);
    const back = JSON.parse(formToJson(form!));
    expect(back).toEqual(JSON.parse(json));
  });

  it("returns null for unparsable text instead of dropping content", () => {
    expect(formFromJson("{nope")).toBeNull();
  });

  it("skips blank keys and paths on serialize", () => {
    const json = formToJson({
      env: [{ key: " ", value: "x" }],
      routes: [{ method: "GET", path: "  ", status: "200", body: "{}" }],
      passthrough: false,
    });
    expect(JSON.parse(json)).toEqual({ env: {}, routes: {} });
  });
});

import { describe, expect, it } from "vitest";
import { formFromJson, formToJson, suggestFromIr, validate } from "../src/mockform";

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

  it("returns null for unparseable text instead of dropping content", () => {
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

describe("validate", () => {
  it("points at the exact field", () => {
    const issues = validate({
      env: [],
      routes: [{ method: "GET", path: "a", status: "9000", body: "{oops" }],
      passthrough: false,
    });
    expect(issues.map((i) => i.field).sort()).toEqual(["body", "path", "status"]);
    expect(issues.every((i) => i.index === 0)).toBe(true);
  });

  it("accepts a well-formed row", () => {
    expect(
      validate({
        env: [],
        routes: [{ method: "POST", path: "/x", status: "404", body: '{"a":1}' }],
        passthrough: true,
      }),
    ).toEqual([]);
  });
});

describe("suggestFromIr", () => {
  it("finds @http operations and @env keys", () => {
    const ir = JSON.stringify({
      modules: [
        {
          shapes: [
            {
              fields: [{ sources: [{ env: "API_TOKEN" }] }],
              operations: [
                { traits: [{ id: "http", value: { method: "GET", path: "/users/{username}" } }] },
              ],
            },
          ],
        },
      ],
    });
    const suggestions = suggestFromIr(ir);
    expect(suggestions).toHaveLength(1);
    expect(suggestions[0]).toMatchObject({ method: "GET", path: "/users/{username}" });
    expect(suggestions[0].envKeys).toContain("API_TOKEN");
  });

  it("is empty on garbage", () => {
    expect(suggestFromIr("nope")).toEqual([]);
  });
});

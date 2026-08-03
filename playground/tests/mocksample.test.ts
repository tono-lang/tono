import { describe, expect, it } from "vitest";
import { opCatalog, routeMatches } from "../src/mocksample";

const IR = JSON.stringify({
  modules: [
    {
      shapes: [
        {
          id: "playground#status",
          kind: "enum",
          values: [{ name: "active" }, { name: "settled" }],
        },
        {
          id: "playground#account",
          kind: "structure",
          members: [
            { name: "id", required: true, target: { prim: "uuid" } },
            { name: "balance", required: true, target: { prim: "i64" } },
            { name: "state", required: true, target: { ref: "playground#status" } },
            { name: "note", required: false, target: { prim: "string" } },
            { name: "tags", required: true, target: { list: { prim: "string" } } },
          ],
        },
        {
          id: "playground#client",
          kind: "entry",
          fields: [{ sources: [{ env: "API_ENDPOINT" }] }],
          operations: [
            {
              id: "playground#client.get_user",
              output: { ref: "playground#account" },
              traits: [{ id: "http", value: { method: "GET", path: "/users/{username}" } }],
            },
            {
              id: "playground#client.local_note",
              output: { ref: "playground#account" },
              traits: [],
            },
          ],
        },
      ],
      extensions: [
        { kind: "impl", name: "client.local_note", bindings: { ts: "x#f", go: "y#F" } },
      ],
    },
  ],
});

describe("opCatalog", () => {
  it("lists every op: http ones with a route, bespoke ones with impl langs", () => {
    const ops = opCatalog(IR);
    expect(ops).toHaveLength(2);
    expect(ops[0]).toMatchObject({
      name: "get_user",
      route: { method: "GET", path: "/users/{username}" },
    });
    expect(ops[0].envKeys).toContain("API_ENDPOINT");
    expect(ops[1]).toMatchObject({ name: "local_note", route: null });
    expect(ops[1].implLangs.sort()).toEqual(["go", "ts"]);
    const body = JSON.parse(ops[0].sampleBody);
    // Wire conventions hold: i64 as string, enum as its first value, optional absent.
    expect(body.balance).toBe("0");
    expect(body.state).toBe("active");
    expect(body).not.toHaveProperty("note");
    expect(body.tags).toEqual(["hello"]);
    expect(typeof body.id).toBe("string");
  });

  it("is empty on garbage", () => {
    expect(opCatalog("nope")).toEqual([]);
  });
});

describe("routeMatches", () => {
  it("matches labels against concrete segments", () => {
    expect(routeMatches("/users/{username}", "/users/gandarfh")).toBe(true);
    expect(routeMatches("/users/{username}", "/users/")).toBe(false);
    expect(routeMatches("/users/{username}", "/users/a/b")).toBe(false);
    expect(routeMatches("/account", "/account")).toBe(true);
    expect(routeMatches("/account", "/other")).toBe(false);
  });
});

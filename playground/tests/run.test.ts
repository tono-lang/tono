import { describe, expect, it } from "vitest";
import { harnessSource, matchRoute, parseRunConfig } from "../src/run";

describe("parseRunConfig", () => {
  it("fills missing sections with empty defaults", () => {
    expect(parseRunConfig("{}")).toEqual({ routes: {}, env: {} });
  });

  it("keeps declared routes and env", () => {
    const config = parseRunConfig('{"env":{"A":"1"},"routes":{"GET /x":{"status":204}}}');
    expect(config).toEqual({ env: { A: "1" }, routes: { "GET /x": { status: 204 } } });
  });

  it("returns the parse error as a string", () => {
    const result = parseRunConfig("{nope");
    expect(typeof result).toBe("string");
    expect(result).toContain("mocks.json");
  });
});

describe("matchRoute", () => {
  const routes = { "GET /account": { status: 200, body: { ok: true } } };

  it("matches method plus pathname, ignoring host and query", () => {
    const hit = matchRoute(routes, "get", "https://api.example.com/account?x=1");
    expect(hit).toEqual({ status: 200, body: { ok: true }, headers: {} });
  });

  it("misses a different method or path", () => {
    expect(matchRoute(routes, "POST", "https://api.example.com/account")).toBeNull();
    expect(matchRoute(routes, "GET", "https://api.example.com/other")).toBeNull();
  });

  it("defaults status and body", () => {
    const hit = matchRoute({ "GET /x": {} }, "GET", "/x");
    expect(hit).toEqual({ status: 200, body: {}, headers: {} });
  });
});

describe("harnessSource", () => {
  it("embeds the config and patches fetch, console, and process.env", () => {
    const source = harnessSource({ env: { API_TOKEN: "t" }, routes: { "GET /a": { body: 1 } } });
    expect(source).toContain('"API_TOKEN":"t"');
    expect(source).toContain('"GET /a"');
    expect(source).toContain("globalThis.fetch");
    expect(source).toContain("globalThis.process");
    expect(source).toContain("unhandledrejection");
  });
});

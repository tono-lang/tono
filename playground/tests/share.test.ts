import { describe, expect, it } from "vitest";
import { decodeShareHash, encodeShareHash } from "../src/share";

describe("share hash", () => {
  it("round-trips source and target", async () => {
    const source = 'pub enum status { active }\n// comment with acentuação\n';
    const hash = await encodeShareHash({ source, target: "rust" });
    expect(hash.startsWith("#code=")).toBe(true);
    const decoded = await decodeShareHash(hash);
    expect(decoded).toEqual({ source, target: "rust" });
  });

  it("defaults an unknown target to ts", async () => {
    const hash = await encodeShareHash({ source: "x", target: "ts" });
    const tampered = hash.replace(/target=ts$/, "target=cobol");
    const decoded = await decodeShareHash(tampered);
    expect(decoded?.target).toBe("ts");
  });

  it("returns null for an empty or foreign hash", async () => {
    expect(await decodeShareHash("")).toBeNull();
    expect(await decodeShareHash("#other=1")).toBeNull();
  });

  it("returns null for corrupted payloads", async () => {
    expect(await decodeShareHash("#code=%%%%")).toBeNull();
    expect(await decodeShareHash("#code=abcd")).toBeNull();
  });

  it("round-trips the open file when present", async () => {
    const hash = await encodeShareHash({
      source: "x",
      target: "go",
      file: "go/playground/types.go",
    });
    const decoded = await decodeShareHash(hash);
    expect(decoded?.file).toBe("go/playground/types.go");
    const bare = await decodeShareHash(await encodeShareHash({ source: "x", target: "go" }));
    expect(bare?.file).toBeUndefined();
  });

  it("round-trips run panel content when present", async () => {
    const hash = await encodeShareHash({
      source: "x",
      target: "ts",
      run: 'console.log("hi");\n',
      mocks: '{"env":{}}\n',
    });
    const decoded = await decodeShareHash(hash);
    expect(decoded?.run).toBe('console.log("hi");\n');
    expect(decoded?.mocks).toBe('{"env":{}}\n');
    const bare = await decodeShareHash(await encodeShareHash({ source: "x", target: "ts" }));
    expect(bare?.run).toBeUndefined();
    expect(bare?.mocks).toBeUndefined();
  });

  it("produces URL-safe output", async () => {
    const source = Array.from({ length: 64 }, (_, i) => `struct s${i} { f: string }`).join("\n");
    const hash = await encodeShareHash({ source, target: "go" });
    expect(hash).toMatch(/^#code=[A-Za-z0-9_-]+&target=go$/);
  });
});

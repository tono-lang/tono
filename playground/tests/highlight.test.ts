import { beforeAll, describe, expect, it } from "vitest";
import { highlight, initTonoHighlighter } from "../src/highlight";

const here = new URL(".", import.meta.url);

beforeAll(async () => {
  // web-tree-sitter loads these as plain filesystem paths under Node; the
  // `?url` defaults `initTonoHighlighter` otherwise uses are server paths
  // meant for the browser's fetch, which don't resolve outside one.
  await initTonoHighlighter({
    treeSitter: new URL("../node_modules/web-tree-sitter/tree-sitter.wasm", here).pathname,
    tono: new URL("../src/generated/tree-sitter-tono.wasm", here).pathname,
  });
});

function classesAt(source: string, text: string): string[] {
  const at = source.indexOf(text);
  if (at === -1) throw new Error(`fixture bug: ${JSON.stringify(text)} not found in source`);
  return highlight(source)
    .filter((r) => r.from === at && r.to === at + text.length)
    .map((r) => r.cls);
}

describe("highlight", () => {
  it("paints keywords, type names, and primitive types", () => {
    const source = "pub struct card { last4: string }";
    expect(classesAt(source, "pub")).toEqual(["tono-keyword"]);
    expect(classesAt(source, "struct")).toEqual(["tono-keyword"]);
    expect(classesAt(source, "card")).toEqual(["tono-type"]);
    expect(classesAt(source, "string")).toEqual(["tono-type"]);
  });

  it("paints numbers and comments", () => {
    const source = "// a doc comment\npub enum status { active = 1 }\n";
    expect(classesAt(source, "// a doc comment")).toEqual(["tono-comment"]);
    expect(classesAt(source, "1")).toEqual(["tono-number"]);
  });

  it("paints an attribute, its name, and a string argument", () => {
    const source = '@doc("hi")\npub struct card { last4: string }';
    expect(classesAt(source, "@")).toEqual(["tono-attribute"]);
    expect(classesAt(source, "doc")).toEqual(["tono-attribute"]);
    expect(classesAt(source, '"hi"')).toEqual(["tono-string"]);
  });

  it("leaves plain identifiers and punctuation unstyled", () => {
    const source = "pub struct card { last4: string }";
    expect(classesAt(source, "last4")).toEqual([]);
    expect(classesAt(source, "{")).toEqual([]);
  });

  it("keeps ranges sorted and non-overlapping for the editor's range builder", () => {
    const source = '@doc("hi")\npub struct card { last4: string }\npub enum status { active }\n';
    const ranges = highlight(source);
    const starts = ranges.map((r) => r.from);
    expect(starts).toEqual([...starts].sort((a, b) => a - b));
    for (let i = 1; i < ranges.length; i++) {
      expect(ranges[i].from).toBeGreaterThanOrEqual(ranges[i - 1].to);
    }
  });
});

import { describe, expect, it } from "vitest";
import { highlightRanges } from "../src/highlight";
import type { TokenSpan } from "../src/types";

const tok = (family: TokenSpan["family"], startOffset: number, endOffset: number): TokenSpan => ({
  family,
  startOffset,
  endOffset,
});

describe("highlightRanges", () => {
  it("maps token families to classes and skips unstyled ones", () => {
    const source = "pub struct card";
    const ranges = highlightRanges(source, [
      tok("keyword", 0, 3),
      tok("keyword", 4, 10),
      tok("ident", 11, 15),
    ]);
    expect(ranges).toEqual([
      { from: 0, to: 3, cls: "tono-keyword" },
      { from: 4, to: 10, cls: "tono-keyword" },
    ]);
  });

  it("recovers comments from gaps between tokens", () => {
    const source = "pub // trailing\nident";
    const ranges = highlightRanges(source, [tok("keyword", 0, 3), tok("ident", 16, 21)]);
    expect(ranges).toContainEqual({ from: 4, to: 15, cls: "tono-comment" });
  });

  it("recovers a comment after the last token", () => {
    const source = "pub // done";
    const ranges = highlightRanges(source, [tok("keyword", 0, 3)]);
    expect(ranges).toContainEqual({ from: 4, to: 11, cls: "tono-comment" });
  });

  it("never scans inside string tokens for comments", () => {
    const source = '@doc("http://x") a';
    const ranges = highlightRanges(source, [
      tok("attribute", 0, 1),
      tok("ident", 1, 4),
      tok("punct", 4, 5),
      tok("string", 5, 15),
      tok("punct", 15, 16),
      tok("ident", 17, 18),
    ]);
    expect(ranges.filter((r) => r.cls === "tono-comment")).toEqual([]);
  });

  it("paints the identifier following @ as part of the attribute", () => {
    const source = "@doc x";
    const ranges = highlightRanges(source, [
      tok("attribute", 0, 1),
      tok("ident", 1, 4),
      tok("ident", 5, 6),
    ]);
    expect(ranges).toEqual([
      { from: 0, to: 1, cls: "tono-attribute" },
      { from: 1, to: 4, cls: "tono-attribute" },
    ]);
  });

  it("keeps ranges sorted for the editor's range builder", () => {
    const source = "pub // c\npub";
    const ranges = highlightRanges(source, [tok("keyword", 0, 3), tok("keyword", 9, 12)]);
    const starts = ranges.map((r) => r.from);
    expect(starts).toEqual([...starts].sort((a, b) => a - b));
  });
});

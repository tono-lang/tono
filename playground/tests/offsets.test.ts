import { describe, expect, it } from "vitest";
import { byteToCharMapper } from "../src/offsets";

describe("byteToCharMapper", () => {
  it("is the identity for ASCII", () => {
    const toChar = byteToCharMapper("struct x");
    expect(toChar(0)).toBe(0);
    expect(toChar(7)).toBe(7);
    expect(toChar(8)).toBe(8);
  });

  it("collapses multi-byte sequences to one char index", () => {
    // "é" is two bytes (0xC3 0xA9) but one UTF-16 unit.
    const toChar = byteToCharMapper("é x");
    expect(toChar(0)).toBe(0);
    expect(toChar(1)).toBe(0);
    expect(toChar(2)).toBe(1);
    expect(toChar(3)).toBe(2);
  });

  it("handles astral characters (4 bytes, 2 UTF-16 units)", () => {
    const source = "\u{1F600}x";
    const toChar = byteToCharMapper(source);
    expect(toChar(0)).toBe(0);
    expect(toChar(3)).toBe(0);
    expect(toChar(4)).toBe(2);
    expect(toChar(5)).toBe(3);
  });

  it("clamps out-of-range offsets", () => {
    const toChar = byteToCharMapper("ab");
    expect(toChar(-1)).toBe(0);
    expect(toChar(99)).toBe(2);
  });
});

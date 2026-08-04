import { describe, expect, it } from "vitest";
import { enclosingDecl, findOccurrences } from "../src/xref";
import type { DeclInfo } from "../src/compiler";

const d = (name: string, nameStart: number, nameEnd: number, kind = "struct"): DeclInfo => ({
  name,
  kind,
  nameStart,
  nameEnd,
});

describe("enclosingDecl", () => {
  const decls = [d("card", 11, 15), d("client", 40, 46), d("client.get", 60, 63, "op")];

  it("resolves the declaration whose name starts before the cursor", () => {
    expect(enclosingDecl(decls, 20)?.name).toBe("card");
    expect(enclosingDecl(decls, 50)?.name).toBe("client");
  });

  it("prefers a nested op over its enclosing entry", () => {
    expect(enclosingDecl(decls, 70)?.name).toBe("client.get");
  });

  it("returns null before the first declaration", () => {
    expect(enclosingDecl(decls, 5)).toBeNull();
    expect(enclosingDecl([], 5)).toBeNull();
  });
});

describe("findOccurrences", () => {
  it("finds word-boundary matches only", () => {
    const text = "type Card struct { }\nfunc NewCard() Card { }\n// Cardigan";
    const hits = findOccurrences(text, "Card");
    expect(hits).toEqual([
      { from: 5, to: 9 },
      { from: 36, to: 40 },
    ]);
  });

  it("does not match inside identifiers", () => {
    expect(findOccurrences("StatusActive Status_ MyStatus", "Status")).toEqual([]);
  });

  it("handles empty ident and no matches", () => {
    expect(findOccurrences("abc", "")).toEqual([]);
    expect(findOccurrences("abc", "xyz")).toEqual([]);
  });
});

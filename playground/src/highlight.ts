/* Highlighting for .tono derived from the frontend's own lexer: the shim
   returns real token spans, so there is no second grammar to keep in sync with
   the parser. The lexer discards comments, so those are recovered here by
   scanning only the gaps between tokens (a "//" inside a string literal is
   inside a token and never scanned). */
import type { TokenSpan } from "./types";

export interface HighlightRange {
  from: number;
  to: number;
  cls: string;
}

const FAMILY_CLASS: Record<string, string | null> = {
  keyword: "tono-keyword",
  type: "tono-type",
  string: "tono-string",
  number: "tono-number",
  attribute: "tono-attribute",
  ident: null,
  punct: null,
};

function commentRanges(source: string, from: number, to: number): HighlightRange[] {
  const ranges: HighlightRange[] = [];
  let i = from;
  while (i < to - 1) {
    if (source[i] === "/" && source[i + 1] === "/") {
      let end = source.indexOf("\n", i);
      if (end === -1 || end > to) end = to;
      ranges.push({ from: i, to: end, cls: "tono-comment" });
      i = end;
    } else {
      i++;
    }
  }
  return ranges;
}

/* Tokens arrive with offsets already mapped to string indices and sorted in
   source order. */
export function highlightRanges(source: string, tokens: TokenSpan[]): HighlightRange[] {
  const ranges: HighlightRange[] = [];
  let prevEnd = 0;
  let prevFamily: string | null = null;
  for (const token of tokens) {
    ranges.push(...commentRanges(source, prevEnd, token.startOffset));
    /* The name after "@" reads as one attribute: paint the identifier with the
       marker. */
    const cls =
      token.family === "ident" &&
      prevFamily === "attribute" &&
      token.startOffset === prevEnd
        ? "tono-attribute"
        : FAMILY_CLASS[token.family] ?? null;
    if (cls && token.endOffset > token.startOffset) {
      ranges.push({ from: token.startOffset, to: token.endOffset, cls });
    }
    prevEnd = token.endOffset;
    prevFamily = token.family;
  }
  ranges.push(...commentRanges(source, prevEnd, source.length));
  return ranges;
}

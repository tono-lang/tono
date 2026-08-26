/* Highlighting for .tono via tree-sitter-tono, the same grammar the CLI's
   preview panes and the LSP already use, instead of a second highlighter
   derived from the frontend's own lexer. The grammar and its query are built
   fresh from the pinned rev by build-compiler.sh, so nothing about the
   grammar's shape is encoded here. */
import { Language, Parser, Query, type QueryCapture } from "web-tree-sitter";
import treeSitterWasmUrl from "web-tree-sitter/tree-sitter.wasm?url";
import tonoWasmUrl from "./generated/tree-sitter-tono.wasm?url";
import highlightsQuerySource from "./generated/tono-highlights.scm?raw";

export interface HighlightRange {
  from: number;
  to: number;
  cls: string;
}

/* The grammar's capture taxonomy is much richer than the editor's CSS. Map
   each capture down to one of the existing classes, or drop it (identifiers,
   punctuation, and operators stay unstyled, as they did under the old
   lexer-based highlighter). Booleans and the null/None builtins read as
   keyword-like literals even though the query names them separately. */
const CAPTURE_CLASS: Record<string, string> = {
  keyword: "tono-keyword",
  "keyword.import": "tono-keyword",
  "keyword.modifier": "tono-keyword",
  "keyword.conditional": "tono-keyword",
  boolean: "tono-keyword",
  "constant.builtin": "tono-keyword",
  type: "tono-type",
  "type.builtin": "tono-type",
  "type.definition": "tono-type",
  "type.parameter": "tono-type",
  string: "tono-string",
  "string.special": "tono-string",
  "string.escape": "tono-string",
  number: "tono-number",
  "number.float": "tono-number",
  attribute: "tono-attribute",
  comment: "tono-comment",
};

let parser: Parser | null = null;
let query: Query | null = null;

/* The two wasm locations default to the app's own bundled assets; tests
   override them with plain filesystem paths, since web-tree-sitter loads a
   `?url` string (a server path meant for the browser's fetch) as a literal
   path under Node, where no such server exists. */
export async function initTonoHighlighter(
  wasm: { treeSitter?: string; tono?: string } = {},
): Promise<void> {
  await Parser.init({ locateFile: () => wasm.treeSitter ?? treeSitterWasmUrl });
  const language = await Language.load(wasm.tono ?? tonoWasmUrl);
  parser = new Parser();
  parser.setLanguage(language);
  query = new Query(language, highlightsQuerySource);
}

/* A later pattern in the query wins over an earlier one on the same range
   (the query's own convention: e.g. a handle-method-call target overrides the
   generic field reading it also matches), so a plain last-write-wins map
   replicates it. The subsequent overlap filter exists only for
   CodeMirror's RangeSetBuilder, which rejects ranges that nest inside one
   another; the query's captures are all leaf-level tokens, so this should
   never trigger in practice, but the check is cheap insurance against a
   crash if a future query revision captures a wider node. */
function toRanges(captures: QueryCapture[]): HighlightRange[] {
  const byRange = new Map<string, HighlightRange>();
  for (const capture of captures) {
    const cls = CAPTURE_CLASS[capture.name];
    if (!cls) continue;
    const { startIndex: from, endIndex: to } = capture.node;
    if (to <= from) continue;
    byRange.set(`${from}:${to}`, { from, to, cls });
  }
  const sorted = Array.from(byRange.values()).sort((a, b) => a.from - b.from || a.to - b.to);
  const ranges: HighlightRange[] = [];
  let end = 0;
  for (const range of sorted) {
    if (range.from < end) continue;
    ranges.push(range);
    end = range.to;
  }
  return ranges;
}

export function highlight(source: string): HighlightRange[] {
  if (!parser || !query) throw new Error("initTonoHighlighter() must resolve before highlight()");
  const tree = parser.parse(source);
  if (!tree) return [];
  try {
    return toRanges(query.captures(tree.rootNode));
  } finally {
    tree.delete();
  }
}

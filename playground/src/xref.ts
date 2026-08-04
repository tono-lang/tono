/* Pure helpers behind the cross-target highlight: resolve which declaration
   the cursor sits in, and where a target-language identifier occurs in a
   generated file. Identifier names come from the backend's own naming (the
   wasm `symbols` export), never derived here. */
import type { DeclInfo } from "./compiler";

/* The surface AST carries only name spans, so a declaration's extent is
   [its name, the next declaration's name): the same heuristic the LSP uses.
   Offsets here are in the same space as `offset` (the caller converts). */
export function enclosingDecl(decls: DeclInfo[], offset: number): DeclInfo | null {
  let found: DeclInfo | null = null;
  for (const d of decls) {
    if (d.nameStart <= offset) {
      /* A nested op starts after its entry, so the later start wins. */
      if (!found || d.nameStart >= found.nameStart) found = d;
    }
  }
  return found;
}

export interface Occurrence {
  from: number;
  to: number;
}

/* Word-boundary occurrences of an identifier. Identifiers are ASCII
   [A-Za-z0-9_] in every target, so the boundary test is a simple
   character-class check, no regex escaping concerns. */
export function findOccurrences(text: string, ident: string): Occurrence[] {
  if (!ident) return [];
  const isWord = (c: string | undefined): boolean => c !== undefined && /[A-Za-z0-9_]/.test(c);
  const out: Occurrence[] = [];
  let i = text.indexOf(ident);
  while (i !== -1) {
    const before = text[i - 1];
    const after = text[i + ident.length];
    if (!isWord(before) && !isWord(after)) {
      out.push({ from: i, to: i + ident.length });
    }
    i = text.indexOf(ident, i + ident.length);
  }
  return out;
}

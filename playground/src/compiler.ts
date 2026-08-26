/* Typed facade over the two embedded compiler halves: the js_of_ocaml build of
   the OCaml frontend (parse, typecheck, IR, plus the LSP's analysis core) and
   the wasm-bindgen build of the Rust codegen backend. Loading is async because
   the wasm module streams in. */
import initBackend, {
  generate as backendGenerate,
  ir_version as backendIrVersion,
  symbols as backendSymbols,
} from "./generated/backend/tono_playground_backend";
import "./generated/tono_frontend";
import type { CompileResult, Diagnostic, GeneratedFile, Target } from "./types";

export interface DeclInfo {
  name: string;
  kind: string;
  /* Byte offsets of the declaration's name in the source. */
  nameStart: number;
  nameEnd: number;
}

export interface SymbolInfo {
  id: string;
  ident: string;
  kind: string;
}

export interface CompletionInfo {
  label: string;
  detail: string | null;
  insertText: string | null;
  documentation: string | null;
}

/* LSP convention: 0-based line, UTF-16 character column. */
export interface LspPosition {
  line: number;
  character: number;
}

export interface LspRange {
  start: LspPosition;
  end: LspPosition;
}

export interface HoverInfo {
  contents: string;
  range: LspRange | null;
}

interface RawFrontend {
  compile(src: string, moduleName: string): { ir: string | null; diagnostics: Diagnostic[] };
  formatSource(src: string): { formatted: string | null; error: string | null };
  decls(src: string): DeclInfo[];
  completionsAt(src: string, line: number, character: number): CompletionInfo[];
  hoverAt(src: string, line: number, character: number): HoverInfo | null;
  definitionAt(src: string, line: number, character: number): LspRange | null;
  irVersion(): number;
  version(): string;
}

declare global {
  // eslint-disable-next-line no-var
  var tonoFrontend: RawFrontend;
}

export interface Compiler {
  compile(source: string, moduleName: string): CompileResult;
  formatSource(source: string): { formatted: string | null; error: string | null };
  decls(source: string): DeclInfo[];
  completionsAt(source: string, line: number, character: number): CompletionInfo[];
  hoverAt(source: string, line: number, character: number): HoverInfo | null;
  definitionAt(source: string, line: number, character: number): LspRange | null;
  generate(ir: string, target: Target): GeneratedFile[];
  symbols(ir: string, target: Target): SymbolInfo[];
  frontendVersion: string;
  irVersion: number;
}

export async function loadCompiler(): Promise<Compiler> {
  await initBackend();
  const frontend = globalThis.tonoFrontend;
  const frontendIr = frontend.irVersion();
  const backendIr = backendIrVersion();
  if (frontendIr !== backendIr) {
    throw new Error(
      `IR version mismatch: frontend ${frontendIr}, backend ${backendIr}`,
    );
  }
  return {
    compile: (source, moduleName) => {
      const raw = frontend.compile(source, moduleName);
      return { ir: raw.ir, diagnostics: Array.from(raw.diagnostics) };
    },
    formatSource: (source) => frontend.formatSource(source),
    decls: (source) => Array.from(frontend.decls(source)),
    completionsAt: (source, line, character) =>
      Array.from(frontend.completionsAt(source, line, character)),
    hoverAt: (source, line, character) => frontend.hoverAt(source, line, character),
    definitionAt: (source, line, character) =>
      frontend.definitionAt(source, line, character),
    generate: (ir, target) => {
      const out = JSON.parse(backendGenerate(ir, target)) as {
        files: GeneratedFile[];
      };
      return out.files;
    },
    symbols: (ir, target) => JSON.parse(backendSymbols(ir, target)) as SymbolInfo[],
    frontendVersion: frontend.version(),
    irVersion: frontendIr,
  };
}

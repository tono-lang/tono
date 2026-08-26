export type Severity = "error" | "warning";

export interface Diagnostic {
  message: string;
  code: string | null;
  severity: Severity;
  line: number;
  col: number;
  /* Byte offsets into the UTF-8 source; map with offsets.ts before use. */
  startOffset: number;
  endOffset: number;
}

export interface CompileResult {
  ir: string | null;
  diagnostics: Diagnostic[];
}

export interface GeneratedFile {
  path: string;
  text: string;
}

export type Target = "ts" | "rust" | "go";

export const TARGETS: { id: Target; label: string }[] = [
  { id: "ts", label: "TypeScript" },
  { id: "rust", label: "Rust" },
  { id: "go", label: "Go" },
];

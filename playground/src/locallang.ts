/* Completions for the Go and Rust run editors. Full language servers would
   need gopls and rust-analyzer proxied through the CLI; until then this offers
   what the playground authoritatively knows: the generated SDK's identifiers
   for the target (from the backend's own naming, so casing and @rename are
   exact) plus the language's keywords and the snippet-relevant idioms. */
import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import type { SymbolInfo } from "./compiler";

const GO_WORDS = [
  "package", "import", "func", "var", "const", "type", "struct", "interface",
  "map", "range", "if", "else", "for", "switch", "case", "default", "return",
  "defer", "go", "select", "break", "continue", "nil", "true", "false",
  "error", "string", "int", "int32", "int64", "bool", "byte",
  "fmt.Println", "fmt.Printf", "context.Background()", "panic",
];

const RUST_WORDS = [
  "fn", "let", "mut", "use", "pub", "struct", "enum", "impl", "trait",
  "match", "if", "else", "for", "while", "loop", "return", "async", "await",
  "move", "Some", "None", "Ok", "Err", "String", "Vec", "println!",
  "expect", "unwrap", "#[tokio::main]",
];

const KIND_TYPE: Record<string, string> = {
  struct: "class",
  union: "class",
  enum: "enum",
  entry: "class",
  op: "method",
  config: "interface",
  service: "interface",
};

export function localLangCompletion(
  lang: "go" | "rust",
  getSymbols: () => SymbolInfo[],
  getModule: () => string,
): Extension {
  const words = lang === "go" ? GO_WORDS : RUST_WORDS;
  const source = (ctx: CompletionContext): CompletionResult | null => {
    const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_!.:()\[\]]*$/);
    if (!word && !ctx.explicit) return null;
    const moduleName = getModule();
    const options = [
      ...words.map((w) => ({ label: w, type: "keyword" as const })),
      ...(lang === "go"
        ? [{ label: `${moduleName} "tono_preview/${moduleName}"`, type: "namespace", detail: "import" }]
        : [{ label: `tono_run::${moduleName}`, type: "namespace", detail: "crate path" }]),
      ...getSymbols().map((s) => ({
        label: s.ident,
        type: KIND_TYPE[s.kind] ?? "variable",
        detail: `sdk ${s.kind}`,
      })),
    ];
    return { from: word ? word.from : ctx.pos, options, validFor: /^[\w!.:]*$/ };
  };
  return autocompletion({ override: [source] });
}

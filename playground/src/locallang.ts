/* Completions for the Go and Rust run editors. Served by the CLI, the real
   language server answers through /api/complete (gopls, rust-analyzer) with
   the generated SDK on disk, so builtin packages and type members all work.
   Without one (not installed, or a plain static build), the fallback offers
   what the playground authoritatively knows: the SDK's identifiers for the
   target, from the backend's own naming, plus language keywords. */
import { autocompletion, type Completion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
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

/* LSP CompletionItemKind numbers onto CodeMirror completion types. */
const LSP_KIND: Record<number, string> = {
  2: "method",
  3: "function",
  4: "method",
  5: "property",
  6: "variable",
  7: "class",
  8: "interface",
  9: "namespace",
  10: "property",
  13: "enum",
  14: "keyword",
  20: "constant",
  21: "constant",
  22: "class",
  25: "type",
};

interface ServerItem {
  label: string;
  kind: number;
  detail: string;
  documentation: string;
  insertText: string;
}

export function localLangCompletion(
  lang: "go" | "rust",
  getters: {
    symbols: () => SymbolInfo[];
    module: () => string;
    tonoSource: () => string;
  },
): Extension {
  const words = lang === "go" ? GO_WORDS : RUST_WORDS;
  /* One failed probe silences the server path for the session; the static
     fallback keeps answering. */
  let serverDown = false;

  const fallback = (from: number): CompletionResult => {
    const moduleName = getters.module();
    const options: Completion[] = [
      ...words.map((w) => ({ label: w, type: "keyword" })),
      ...(lang === "go"
        ? [{ label: `${moduleName} "tono_preview/${moduleName}"`, type: "namespace", detail: "import" }]
        : [{ label: `tono_run::${moduleName}`, type: "namespace", detail: "crate path" }]),
      ...getters.symbols().map((s) => ({
        label: s.ident,
        type: KIND_TYPE[s.kind] ?? "variable",
        detail: `sdk ${s.kind}`,
      })),
    ];
    return { from, options, validFor: /^[\w!.:]*$/ };
  };

  const source = async (ctx: CompletionContext): Promise<CompletionResult | null> => {
    const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_]*$/);
    const prev = ctx.state.sliceDoc(Math.max(0, ctx.pos - 1), ctx.pos);
    if (!word && !ctx.explicit && prev !== "." && prev !== ":") return null;
    const from = word ? word.from : ctx.pos;
    if (!serverDown) {
      try {
        const line = ctx.state.doc.lineAt(ctx.pos);
        const response = await fetch("api/complete", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({
            target: lang,
            source: getters.tonoSource(),
            module: getters.module(),
            snippet: ctx.state.doc.toString(),
            line: line.number - 1,
            character: ctx.pos - line.from,
          }),
          signal: AbortSignal.timeout(20_000),
        });
        if (response.ok) {
          const data = (await response.json()) as { items: ServerItem[] };
          if (data.items.length > 0) {
            return {
              from,
              options: data.items.map((item) => ({
                label: item.label,
                type: LSP_KIND[item.kind] ?? "text",
                detail: item.detail || undefined,
                apply: item.insertText || item.label,
                ...(item.documentation
                  ? {
                      info: () => {
                        const dom = document.createElement("div");
                        dom.className = "ts-info";
                        const prose = document.createElement("div");
                        prose.className = "ts-info-doc";
                        prose.textContent = item.documentation;
                        dom.append(prose);
                        return { dom };
                      },
                    }
                  : {}),
              })),
              validFor: /^[\w]*$/,
            };
          }
          return fallback(from);
        }
        /* 422: no language server on this machine; anything else is a
           transient failure worth retrying later. */
        if (response.status === 422) serverDown = true;
      } catch {
        serverDown = true;
      }
    }
    return fallback(from);
  };
  return autocompletion({ override: [source], maxRenderedOptions: 40 });
}

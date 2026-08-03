/* CodeMirror wiring: the source editor with lexer-driven highlighting and
   inline diagnostics, plus the read-only output view for generated code. */
import { autocompletion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { go } from "@codemirror/lang-go";
import { javascript } from "@codemirror/lang-javascript";
import { rust } from "@codemirror/lang-rust";
import { bracketMatching, syntaxHighlighting, defaultHighlightStyle } from "@codemirror/language";
import { setDiagnostics, lintGutter, type Diagnostic as CmDiagnostic } from "@codemirror/lint";
import {
  Compartment,
  EditorState,
  RangeSetBuilder,
  StateEffect,
  StateField,
  type Extension,
} from "@codemirror/state";
import { oneDark } from "@codemirror/theme-one-dark";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  ViewUpdate,
  highlightActiveLine,
  hoverTooltip,
  keymap,
  lineNumbers,
  type DecorationSet,
} from "@codemirror/view";
import type { CompletionInfo, HoverInfo, LspPosition, LspRange } from "./compiler";
import { highlightRanges } from "./highlight";
import { byteToCharMapper } from "./offsets";
import type { Diagnostic, Target, TokenSpan } from "./types";

/* CodeMirror positions are UTF-16 offsets; the analysis core speaks LSP
   positions (0-based line, UTF-16 character), so both mappings are exact. */
export function lspPosition(state: EditorState, pos: number): LspPosition {
  const line = state.doc.lineAt(pos);
  return { line: line.number - 1, character: pos - line.from };
}

export function offsetOfLsp(state: EditorState, pos: LspPosition): number {
  const line = state.doc.line(Math.min(pos.line + 1, state.doc.lines));
  return Math.min(line.from + pos.character, line.to);
}

export interface IdeBackend {
  completions(source: string, pos: LspPosition): CompletionInfo[];
  hover(source: string, pos: LspPosition): HoverInfo | null;
  definition(source: string, pos: LspPosition): LspRange | null;
}

function buildDecorations(source: string, tokens: TokenSpan[]): DecorationSet {
  const toChar = byteToCharMapper(source);
  const mapped = tokens.map((t) => ({
    ...t,
    startOffset: toChar(t.startOffset),
    endOffset: toChar(t.endOffset),
  }));
  const builder = new RangeSetBuilder<Decoration>();
  for (const range of highlightRanges(source, mapped)) {
    builder.add(range.from, range.to, Decoration.mark({ class: `cm-${range.cls}` }));
  }
  return builder.finish();
}

function tonoHighlighter(tokenize: (source: string) => TokenSpan[]): Extension {
  return ViewPlugin.fromClass(
    class {
      decorations: DecorationSet;
      constructor(view: EditorView) {
        this.decorations = buildDecorations(view.state.doc.toString(), tokenize(view.state.doc.toString()));
      }
      update(update: ViewUpdate) {
        if (update.docChanged) {
          const source = update.state.doc.toString();
          this.decorations = buildDecorations(source, tokenize(source));
        }
      }
    },
    { decorations: (v) => v.decorations },
  );
}

function ideCompletionSource(ide: IdeBackend) {
  return (ctx: CompletionContext): CompletionResult | null => {
    const word = ctx.matchBefore(/[A-Za-z_][A-Za-z0-9_]*$/);
    if (!word && !ctx.explicit) return null;
    const items = ide.completions(ctx.state.doc.toString(), lspPosition(ctx.state, ctx.pos));
    if (items.length === 0) return null;
    return {
      from: word ? word.from : ctx.pos,
      options: items.map((item) => ({
        label: item.label,
        detail: item.detail ?? undefined,
        apply: item.insertText ?? item.label,
      })),
    };
  };
}

function ideHoverTooltip(ide: IdeBackend): Extension {
  return hoverTooltip((view, pos) => {
    const info = ide.hover(view.state.doc.toString(), lspPosition(view.state, pos));
    if (!info) return null;
    return {
      pos,
      create: () => {
        const dom = document.createElement("div");
        dom.className = "ide-hover";
        dom.textContent = info.contents;
        return { dom };
      },
    };
  });
}

function jumpToDefinition(view: EditorView, ide: IdeBackend, pos: number): boolean {
  const range = ide.definition(view.state.doc.toString(), lspPosition(view.state, pos));
  if (!range) return false;
  const from = offsetOfLsp(view.state, range.start);
  const to = offsetOfLsp(view.state, range.end);
  selectSpan(view, from, to);
  return true;
}

export function createEditor(options: {
  parent: HTMLElement;
  initialSource: string;
  tokenize: (source: string) => TokenSpan[];
  ide: IdeBackend;
  onChange: () => void;
  onCursor: (pos: number) => void;
}): EditorView {
  return new EditorView({
    parent: options.parent,
    state: EditorState.create({
      doc: options.initialSource,
      extensions: [
        lineNumbers(),
        history(),
        bracketMatching(),
        highlightActiveLine(),
        keymap.of([
          {
            key: "F12",
            run: (view) => jumpToDefinition(view, options.ide, view.state.selection.main.head),
          },
          ...defaultKeymap,
          ...historyKeymap,
          indentWithTab,
        ]),
        oneDark,
        lintGutter(),
        tonoHighlighter(options.tokenize),
        autocompletion({ override: [ideCompletionSource(options.ide)] }),
        ideHoverTooltip(options.ide),
        EditorView.domEventHandlers({
          mousedown: (event, view) => {
            if (!(event.metaKey || event.ctrlKey)) return false;
            const pos = view.posAtCoords({ x: event.clientX, y: event.clientY });
            if (pos === null) return false;
            return jumpToDefinition(view, options.ide, pos);
          },
        }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) options.onChange();
          if (update.selectionSet || update.docChanged) {
            options.onCursor(update.state.selection.main.head);
          }
        }),
        EditorView.lineWrapping,
      ],
    }),
  });
}

export function applyDiagnostics(view: EditorView, source: string, diagnostics: Diagnostic[]): void {
  const toChar = byteToCharMapper(source);
  const cmDiags: CmDiagnostic[] = diagnostics.map((d) => {
    const from = Math.min(toChar(d.startOffset), source.length);
    const to = Math.max(from, Math.min(toChar(d.endOffset), source.length));
    return {
      from,
      to: to === from && from < source.length ? from + 1 : to,
      severity: d.severity,
      message: d.code ? `${d.code}: ${d.message}` : d.message,
    };
  });
  view.dispatch(setDiagnostics(view.state, cmDiags));
}

export function selectSpan(view: EditorView, from: number, to: number): void {
  view.dispatch({
    selection: { anchor: from, head: to },
    scrollIntoView: true,
  });
  view.focus();
}

const outputLanguage = new Compartment();

function languageFor(path: string, target: Target): Extension {
  if (path.endsWith(".ts")) return javascript({ typescript: true });
  if (path.endsWith(".rs")) return rust();
  if (path.endsWith(".go")) return go();
  /* package.json and friends: no grammar, plain text is fine. */
  void target;
  return [];
}

/* A small editable editor for the Run panel (user snippet, mocks). */
export function createMiniEditor(options: {
  parent: HTMLElement;
  doc: string;
  lang: "ts" | "json";
  onChange?: () => void;
}): EditorView {
  return new EditorView({
    parent: options.parent,
    state: EditorState.create({
      doc: options.doc,
      extensions: [
        lineNumbers(),
        history(),
        bracketMatching(),
        keymap.of([...defaultKeymap, ...historyKeymap, indentWithTab]),
        oneDark,
        options.lang === "ts" ? javascript({ typescript: true }) : [],
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        EditorView.updateListener.of((update) => {
          if (update.docChanged) options.onChange?.();
        }),
        EditorView.lineWrapping,
      ],
    }),
  });
}

/* Cross-target highlight: ranges in the generated text that belong to the
   declaration under the cursor in the source editor. */
const setXref = StateEffect.define<{ from: number; to: number }[]>();
const xrefMark = Decoration.mark({ class: "cm-xref" });

const xrefField = StateField.define<DecorationSet>({
  create: () => Decoration.none,
  update(deco, tr) {
    let next = deco.map(tr.changes);
    for (const effect of tr.effects) {
      if (effect.is(setXref)) {
        next = Decoration.set(effect.value.map((r) => xrefMark.range(r.from, r.to)));
      }
    }
    return next;
  },
  provide: (f) => EditorView.decorations.from(f),
});

export interface OutputView {
  setContent(path: string, text: string, target: Target): void;
  highlight(ranges: { from: number; to: number }[]): void;
}

export function createOutputView(
  parent: HTMLElement,
  options?: { onSymbolClick?: (word: string) => void },
): OutputView {
  const view = new EditorView({
    parent,
    state: EditorState.create({
      doc: "",
      extensions: [
        lineNumbers(),
        oneDark,
        syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
        outputLanguage.of([]),
        xrefField,
        EditorState.readOnly.of(true),
        EditorView.editable.of(false),
        EditorView.domEventHandlers({
          mousedown: (event, v) => {
            if (!(event.metaKey || event.ctrlKey) || !options?.onSymbolClick) return false;
            const pos = v.posAtCoords({ x: event.clientX, y: event.clientY });
            if (pos === null) return false;
            const range = v.state.wordAt(pos);
            if (!range) return false;
            options.onSymbolClick(v.state.sliceDoc(range.from, range.to));
            return true;
          },
        }),
      ],
    }),
  });
  return {
    setContent(path, text, target) {
      view.dispatch({
        changes: { from: 0, to: view.state.doc.length, insert: text },
        effects: outputLanguage.reconfigure(languageFor(path, target)),
      });
    },
    highlight(ranges) {
      const inBounds = ranges.filter((r) => r.to <= view.state.doc.length && r.from < r.to);
      view.dispatch({
        effects: [
          setXref.of(inBounds),
          ...(inBounds.length > 0 ? [EditorView.scrollIntoView(inBounds[0].from, { y: "center" })] : []),
        ],
      });
    },
  };
}

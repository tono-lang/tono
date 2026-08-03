import "./style.css";
import { CAPABILITIES } from "./capabilities";
import { loadCompiler, type Compiler } from "./compiler";
import {
  applyDiagnostics,
  createEditor,
  createMiniEditor,
  createOutputView,
  selectSpan,
} from "./editor";
import { loadTsLang, type TsLang } from "./tslang";
import {
  bundleRun,
  fetchCapabilities,
  parseRunConfig,
  runInWorker,
  runOnServer,
  type RunLine,
} from "./run";
import { DEFAULT_EXAMPLE, EXAMPLES } from "./examples";
import { buildTree, stripTargetDir, type TreeDir } from "./filetree";
import { byteToCharMapper } from "./offsets";
import { DEFAULT_MODULE, sanitizeModuleName } from "./modname";
import { enclosingDecl, findOccurrences } from "./xref";
import { decodeShareHash, encodeShareHash } from "./share";
import { TARGETS, type Diagnostic, type GeneratedFile, type Target } from "./types";

const $ = <T extends HTMLElement>(sel: string): T => {
  const el = document.querySelector<T>(sel);
  if (!el) throw new Error(`missing element: ${sel}`);
  return el;
};

function debounce(fn: () => void, ms: number): () => void {
  let timer: ReturnType<typeof setTimeout> | undefined;
  return () => {
    clearTimeout(timer);
    timer = setTimeout(fn, ms);
  };
}

interface State {
  target: Target;
  moduleName: string;
  files: GeneratedFile[];
  activeFile: number;
  /* File path from a shared link, applied once files exist for its target. */
  pendingFile: string | null;
  /* IR of the last clean compile, for symbol lookups between refreshes. */
  ir: string | null;
}

async function start(): Promise<void> {
  const statusEl = $("#status");
  statusEl.textContent = "Loading compiler...";

  let compiler: Compiler;
  try {
    compiler = await loadCompiler();
  } catch (err) {
    statusEl.textContent = `Failed to load compiler: ${String(err)}`;
    return;
  }

  $("#meta").textContent = `frontend ${compiler.frontendVersion} | IR v${compiler.irVersion}`;

  const shared = await decodeShareHash(location.hash);
  const state: State = {
    target: shared?.target ?? "ts",
    moduleName: sanitizeModuleName(shared?.name ?? DEFAULT_MODULE),
    files: [],
    activeFile: 0,
    pendingFile: shared?.file ?? null,
    ir: null,
  };

  const editor = createEditor({
    parent: $("#editor"),
    initialSource: shared?.source ?? DEFAULT_EXAMPLE.source,
    tokenize: (source) => compiler.tokens(source),
    ide: {
      completions: (src, pos) => compiler.completionsAt(src, pos.line, pos.character),
      hover: (src, pos) => compiler.hoverAt(src, pos.line, pos.character),
      definition: (src, pos) => compiler.definitionAt(src, pos.line, pos.character),
    },
    onChange: () => scheduleRefresh(),
    onCursor: () => scheduleXref(),
  });

  /* Reverse jump: cmd or ctrl click on an identifier in the generated code
     selects the .tono declaration it came from. */
  const output = createOutputView($("#output"), {
    onSymbolClick: (word) => {
      if (!state.ir) return;
      const sym = compiler.symbols(state.ir, state.target).find((s) => s.ident === word);
      if (!sym) return;
      const name = sym.id.split("#")[1] ?? "";
      const src = source();
      const toChar = byteToCharMapper(src);
      const decl = compiler.decls(src).find((d) => d.name === name);
      if (decl) selectSpan(editor, toChar(decl.nameStart), toChar(decl.nameEnd));
    },
  });
  const outputNote = $("#output-note");
  const diagnosticsEl = $("#diagnostics");
  const fileTree = $("#file-tree");
  const activeFileEl = $("#active-file");
  const targetTabs = $("#target-tabs");

  function source(): string {
    return editor.state.doc.toString();
  }

  function renderDiagnosticsPanel(diags: Diagnostic[]): void {
    diagnosticsEl.replaceChildren();
    diagnosticsEl.hidden = diags.length === 0;
    const toChar = byteToCharMapper(source());
    for (const d of diags) {
      const row = document.createElement("button");
      row.className = `diag diag-${d.severity}`;
      row.textContent = `${d.line}:${d.col} ${d.severity}${d.code ? ` ${d.code}` : ""}: ${d.message}`;
      row.addEventListener("click", () =>
        selectSpan(editor, toChar(d.startOffset), toChar(d.endOffset)),
      );
      diagnosticsEl.append(row);
    }
  }

  function renderDir(dir: TreeDir, container: HTMLElement): void {
    for (const sub of dir.dirs) {
      const details = document.createElement("details");
      details.open = true;
      const summary = document.createElement("summary");
      summary.textContent = sub.name;
      details.append(summary);
      renderDir(sub, details);
      container.append(details);
    }
    for (const file of dir.files) {
      const btn = document.createElement("button");
      btn.className = `tree-file${file.index === state.activeFile ? " active" : ""}`;
      btn.textContent = file.name;
      btn.title = state.files[file.index]?.path ?? file.name;
      btn.addEventListener("click", () => {
        state.activeFile = file.index;
        renderOutput();
        scheduleHashUpdate();
      });
      container.append(btn);
    }
  }

  function renderFileTree(): void {
    fileTree.replaceChildren();
    fileTree.hidden = state.files.length === 0;
    const tree = buildTree(state.files.map((f) => stripTargetDir(f.path)));
    renderDir(tree, fileTree);
  }

  function renderOutput(): void {
    renderFileTree();
    const file = state.files[state.activeFile];
    activeFileEl.textContent = file ? stripTargetDir(file.path) : "";
    if (file) output.setContent(file.path, file.text, state.target);
    else output.setContent("empty.txt", "", state.target);
    scheduleXref();
  }

  function showNote(text: string | null): void {
    outputNote.hidden = !text;
    outputNote.textContent = text ?? "";
  }

  function refresh(): void {
    scheduleHashUpdate();
    const src = source();
    const result = compiler.compile(src, state.moduleName);
    applyDiagnostics(editor, src, result.diagnostics);
    renderDiagnosticsPanel(result.diagnostics);

    const errors = result.diagnostics.filter((d) => d.severity === "error").length;
    state.ir = result.ir;
    if (!result.ir) {
      state.files = [];
      renderOutput();
      showNote(`Fix the ${errors} error${errors === 1 ? "" : "s"} to see generated code.`);
      statusEl.textContent = `${errors} error${errors === 1 ? "" : "s"}`;
      return;
    }
    try {
      state.files = compiler.generate(result.ir, state.target);
      if (state.pendingFile) {
        const i = state.files.findIndex((f) => f.path === state.pendingFile);
        if (i !== -1) state.activeFile = i;
        state.pendingFile = null;
      }
      state.activeFile = Math.min(state.activeFile, Math.max(0, state.files.length - 1));
      renderOutput();
      showNote(null);
      const warnings = result.diagnostics.length - errors;
      statusEl.textContent =
        `${state.files.length} file${state.files.length === 1 ? "" : "s"} generated` +
        (warnings > 0 ? ` | ${warnings} warning${warnings === 1 ? "" : "s"}` : "");
    } catch (err) {
      state.files = [];
      renderOutput();
      showNote(String(err));
      statusEl.textContent = "Generation rejected";
    }
    if (!runPanel.hidden) refreshTsLang();
  }

  const scheduleRefresh = debounce(refresh, 200);

  /* Cross-target highlight: the declaration under the cursor lights up its
     occurrences in the open generated file (and dots the tree entries of the
     other files containing it). The identifier comes from the backend's own
     naming, so it always matches what codegen emitted. */
  function updateXref(): void {
    if (!state.ir || state.files.length === 0) return;
    const src = source();
    const toChar = byteToCharMapper(src);
    const decls = compiler
      .decls(src)
      .map((d) => ({ ...d, nameStart: toChar(d.nameStart), nameEnd: toChar(d.nameEnd) }));
    const decl = enclosingDecl(decls, editor.state.selection.main.head);
    let ident: string | null = null;
    if (decl && decl.kind !== "ext") {
      const symbols = compiler.symbols(state.ir, state.target);
      ident = symbols.find((s) => s.id === `${state.moduleName}#${decl.name}`)?.ident ?? null;
    }
    const active = state.files[state.activeFile];
    output.highlight(active && ident ? findOccurrences(active.text, ident) : []);
    fileTree.querySelectorAll<HTMLElement>(".tree-file").forEach((btn) => {
      const path = btn.title;
      const file = state.files.find((f) => f.path === path);
      const hit = file && ident ? findOccurrences(file.text, ident).length > 0 : false;
      btn.classList.toggle("has-xref", hit);
    });
  }

  const scheduleXref = debounce(updateXref, 150);

  function sharedState() {
    return {
      source: source(),
      target: state.target,
      file: state.files[state.activeFile]?.path,
      ...(state.moduleName !== DEFAULT_MODULE ? { name: state.moduleName } : {}),
      ...(runEditors && !runPanel.hidden
        ? {
            run: runEditors.main.state.doc.toString(),
            mocks: runEditors.mocks.state.doc.toString(),
          }
        : {}),
    };
  }

  const scheduleHashUpdate = debounce(() => {
    void encodeShareHash(sharedState()).then((hash) => {
      history.replaceState(null, "", hash);
    });
  }, 500);

  /* Target tabs */
  for (const { id, label } of TARGETS) {
    const tab = document.createElement("button");
    tab.className = `tab tab-target${id === state.target ? " active" : ""}`;
    tab.dataset.target = id;
    tab.textContent = label;
    tab.addEventListener("click", () => {
      state.target = id;
      state.activeFile = 0;
      targetTabs
        .querySelectorAll(".tab-target")
        .forEach((t) => t.classList.toggle("active", (t as HTMLElement).dataset.target === id));
      refresh();
    });
    targetTabs.append(tab);
  }

  /* Module name: folds to canonical snake_case and recompiles, since it
     shapes every generated package and path. */
  const moduleInput = $<HTMLInputElement>("#module-name");
  moduleInput.value = state.moduleName === DEFAULT_MODULE ? "" : state.moduleName;
  moduleInput.addEventListener("change", () => {
    state.moduleName = sanitizeModuleName(moduleInput.value);
    moduleInput.value = state.moduleName === DEFAULT_MODULE ? "" : state.moduleName;
    refresh();
  });

  /* Example picker */
  const picker = $<HTMLSelectElement>("#example-picker");
  const placeholder = document.createElement("option");
  placeholder.textContent = "Examples";
  placeholder.value = "";
  picker.append(placeholder);
  EXAMPLES.forEach((example, i) => {
    const option = document.createElement("option");
    option.value = String(i);
    option.textContent = example.name;
    picker.append(option);
  });
  picker.addEventListener("change", () => {
    const example = EXAMPLES[Number(picker.value)];
    if (!example) return;
    editor.dispatch({
      changes: { from: 0, to: editor.state.doc.length, insert: example.source },
    });
    picker.value = "";
  });

  /* Format */
  $("#format-btn").addEventListener("click", () => {
    const result = compiler.formatSource(source());
    if (result.formatted !== null) {
      editor.dispatch({
        changes: { from: 0, to: editor.state.doc.length, insert: result.formatted },
      });
    } else {
      statusEl.textContent = "Format needs a parseable file";
    }
  });

  /* Share */
  /* Copy the open generated file */
  $("#copy-btn").addEventListener("click", () => {
    const file = state.files[state.activeFile];
    if (!file) return;
    navigator.clipboard.writeText(file.text).then(
      () => (statusEl.textContent = `Copied ${stripTargetDir(file.path)}`),
      () => (statusEl.textContent = "Copy failed"),
    );
  });

  /* Resizable split between editor and output */
  const splitHandle = $("#split-handle");
  const editorPane = $<HTMLElement>(".pane-editor");
  splitHandle.addEventListener("pointerdown", (down) => {
    down.preventDefault();
    splitHandle.classList.add("dragging");
    splitHandle.setPointerCapture(down.pointerId);
    const total = editorPane.parentElement!.getBoundingClientRect();
    const onMove = (move: PointerEvent) => {
      const pct = ((move.clientX - total.left) / total.width) * 100;
      editorPane.style.flexBasis = `${Math.min(85, Math.max(15, pct))}%`;
    };
    const onUp = () => {
      splitHandle.classList.remove("dragging");
      splitHandle.removeEventListener("pointermove", onMove);
      splitHandle.removeEventListener("pointerup", onUp);
    };
    splitHandle.addEventListener("pointermove", onMove);
    splitHandle.addEventListener("pointerup", onUp);
  });

  /* Run panel: bundle the snippet with the generated TS SDK and execute it in
     a sandboxed worker, HTTP mocked. Editors are created on first open so the
     panel costs nothing until used. */
  const runPanel = $("#run-panel");
  const runConsole = $("#run-console");
  let runEditors: { main: ReturnType<typeof createMiniEditor>; mocks: ReturnType<typeof createMiniEditor> } | null =
    null;
  let activeRun: { stop: () => void } | null = null;
  let tsLang: TsLang | null = null;

  /* Feed the language service the SDK as generated right now, so completions
     in the snippet match the code on the right. */
  function refreshTsLang(): void {
    if (!tsLang || !state.ir) return;
    try {
      tsLang.update(compiler.generate(state.ir, "ts"), runtimeSources, state.moduleName);
    } catch {
      /* An SDK that does not generate leaves the last good types in place. */
    }
  }

  /* Recreate the snippet editor with TypeScript intelligence once the
     (megabytes of) language service arrive; the doc is carried over. */
  function ensureTsLang(): void {
    if (tsLang) return;
    void loadTsLang().then((lang) => {
      tsLang = lang;
      refreshTsLang();
      if (runEditors && runLang(runTarget) === "ts") {
        const doc = runEditors.main.state.doc.toString();
        runEditors.main.destroy();
        runEditors.main = createMiniEditor({
          parent: $("#run-editor"),
          doc,
          lang: "ts",
          onChange: () => scheduleHashUpdate(),
          extra: lang.extensions,
        });
      }
    });
  }

  const runtimeSources: Record<string, string> = Object.fromEntries(
    Object.entries(
      import.meta.glob("./generated/runtime-ts/*.ts", {
        eager: true,
        query: "?raw",
        import: "default",
      }),
    ).map(([path, text]) => [path.split("/").pop()!, text as string]),
  );

  const RUN_TEMPLATES: Record<string, { lang: "ts" | "rust" | "go"; doc: (m: string) => string }> = {
    ts: {
      lang: "ts",
      doc: () => `import * as sdk from "sdk";

console.log("SDK exports:", Object.keys(sdk).join(", "));

// With the "HTTP client" example loaded, try:
// const client = new sdk.Client();
// console.log(await client.getAccount());
`,
    },
    rust: {
      lang: "rust",
      doc: (m) => `// The generated SDK is the crate tono_run; the first run compiles its
// dependency tree, so give it a minute.
// With the "HTTP client" example loaded, try:
// use tono_run::${m}::Client;

#[tokio::main]
async fn main() {
    println!("edit main.rs to call the generated SDK");
}
`,
    },
    go: {
      lang: "go",
      doc: (m) => `package main

import "fmt"

// The generated SDK is the module tono_preview; import its packages like
// ${m} "tono_preview/${m}".
func main() {
	fmt.Println("edit main.go to call the generated SDK")
}
`,
    },
  };

  /* "ts" runs in the browser; "local:<lang>" runs on the serving CLI with the
     machine's toolchain. Everything after the prefix is the language, which
     picks the template and the editor grammar. */
  let runTarget = "ts";
  const runLang = (target: string): string => target.replace(/^local:/, "");
  const runTargetSelect = $<HTMLSelectElement>("#run-target");

  function populateRunTargets(serverTargets: string[]): void {
    runTargetSelect.replaceChildren();
    const browser = document.createElement("option");
    browser.value = "ts";
    browser.textContent = "TypeScript (browser)";
    runTargetSelect.append(browser);
    for (const id of serverTargets) {
      const option = document.createElement("option");
      option.value = `local:${id}`;
      option.textContent =
        id === "ts" ? "TypeScript (local toolchain)" : `${id} (local toolchain)`;
      runTargetSelect.append(option);
    }
    runTargetSelect.hidden = serverTargets.length === 0;
  }
  populateRunTargets([]);

  const DEFAULT_MOCKS = `{
  "env": { "API_TOKEN": "demo-token" },
  "routes": {
    "GET /account": {
      "status": 200,
      "body": { "id": "0b8f8f2e-1e64-4c1c-9b6b-2f8d3a6a1c11", "email": "dev@example.com" }
    }
  }
}
`;

  function appendRunLine(line: RunLine): void {
    if (line.kind === "done") return;
    const el = document.createElement("div");
    el.className = `line-${line.kind}`;
    el.textContent = line.kind === "request" ? `-> ${line.text}` : line.text;
    runConsole.append(el);
    runConsole.scrollTop = runConsole.scrollHeight;
  }

  function openRunPanel(runDoc: string, mocksDoc: string): void {
    runPanel.hidden = false;
    const lang = RUN_TEMPLATES[runLang(runTarget)]?.lang ?? "ts";
    if (lang === "ts") ensureTsLang();
    if (!runEditors) {
      runEditors = {
        main: createMiniEditor({
          parent: $("#run-editor"),
          doc: runDoc,
          lang,
          onChange: () => scheduleHashUpdate(),
          extra: lang === "ts" && tsLang ? tsLang.extensions : [],
        }),
        mocks: createMiniEditor({
          parent: $("#mocks-editor"),
          doc: mocksDoc,
          lang: "json",
          onChange: () => scheduleHashUpdate(),
        }),
      };
    }
    scheduleHashUpdate();
  }

  /* Switching the run language replaces the snippet with that language's
     template: each target's main lives in a different language, so carrying
     text across would only produce syntax errors. */
  const RUN_FILENAMES: Record<string, string> = { ts: "main.ts", rust: "main.rs", go: "main.go" };

  runTargetSelect.addEventListener("change", () => {
    runTarget = runTargetSelect.value;
    const lang = runLang(runTarget);
    $("#run-filename").textContent = RUN_FILENAMES[lang] ?? "main.ts";
    const template = RUN_TEMPLATES[lang] ?? RUN_TEMPLATES.ts;
    if (runEditors) {
      runEditors.main.destroy();
      runEditors.main = createMiniEditor({
        parent: $("#run-editor"),
        doc: template.doc(state.moduleName),
        lang: template.lang,
        onChange: () => scheduleHashUpdate(),
        extra: template.lang === "ts" && tsLang ? tsLang.extensions : [],
      });
    }
    if (template.lang === "ts") ensureTsLang();
  });

  $("#run-toggle").addEventListener("click", () => {
    if (runPanel.hidden) openRunPanel(RUN_TEMPLATES[runLang(runTarget)].doc(state.moduleName), DEFAULT_MOCKS);
    else runPanel.hidden = true;
    scheduleHashUpdate();
  });

  /* Served by `tono playground`? Then the machine's toolchains extend the
     run targets beyond the browser's TypeScript. */
  if (CAPABILITIES.run) {
    void fetchCapabilities().then((caps) => {
      if (caps) populateRunTargets(caps.runTargets);
    });
  }

  if (!CAPABILITIES.run) {
    /* The capped (hosted) build previews and shares only; execution belongs
       to the full build the CLI serves. The panel stays in the DOM so a full
       build restoring a shared link still finds it. */
    $("#run-toggle").hidden = true;
  }

  /* A shared link that carried Run content reopens the panel as it was. */
  if (CAPABILITIES.run && (shared?.run !== undefined || shared?.mocks !== undefined)) {
    openRunPanel(shared.run ?? RUN_TEMPLATES[runLang(runTarget)].doc(state.moduleName), shared.mocks ?? DEFAULT_MOCKS);
  }

  $("#run-exec").addEventListener("click", () => {
    if (!runEditors) return;
    activeRun?.stop();
    runConsole.replaceChildren();
    const config = parseRunConfig(runEditors.mocks.state.doc.toString());
    if (typeof config === "string") {
      appendRunLine({ kind: "error", text: config });
      return;
    }
    if (!state.ir) {
      appendRunLine({ kind: "error", text: "fix the .tono errors first" });
      return;
    }
    if (runTarget.startsWith("local:")) {
      const lang = runLang(runTarget);
      appendRunLine({ kind: "log", text: `running ${lang} with the local toolchain...` });
      void runOnServer({
        source: source(),
        target: lang,
        module: state.moduleName,
        snippet: runEditors.main.state.doc.toString(),
        mocks: config,
      }).then((lines) => {
        runConsole.replaceChildren();
        lines.forEach(appendRunLine);
      });
      return;
    }
    let sdkFiles: GeneratedFile[];
    try {
      sdkFiles = compiler.generate(state.ir, "ts");
    } catch (err) {
      appendRunLine({ kind: "error", text: `TypeScript generation failed: ${String(err)}` });
      return;
    }
    appendRunLine({ kind: "log", text: "bundling..." });
    bundleRun({
      userCode: runEditors.main.state.doc.toString(),
      sdkFiles,
      runtimeSources,
      config,
    }).then(
      (bundle) => {
        runConsole.replaceChildren();
        activeRun = runInWorker(bundle, appendRunLine);
      },
      (err: unknown) => {
        runConsole.replaceChildren();
        appendRunLine({ kind: "error", text: String(err) });
      },
    );
  });

  $("#share-btn").addEventListener("click", () => {
    void encodeShareHash(sharedState()).then(async (hash) => {
      const url = `${location.origin}${location.pathname}${hash}`;
      history.replaceState(null, "", hash);
      try {
        await navigator.clipboard.writeText(url);
        statusEl.textContent = "Link copied to clipboard";
      } catch {
        statusEl.textContent = "Copy failed; the URL bar holds the link";
      }
    });
  });

  refresh();
}

void start();

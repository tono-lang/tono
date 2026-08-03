/* The Run panel: bundle the user's snippet against the generated TypeScript
   SDK (and the real @tono/http-runtime-ts sources) with esbuild-wasm, then
   execute the bundle in a sandboxed module worker. HTTP never leaves the
   worker: the harness replaces global fetch with a route table the user
   declares, and seeds process.env for @env fields the same way. */
import type { GeneratedFile } from "./types";

export interface RunConfig {
  /* "METHOD /path" -> canned response. */
  routes: Record<string, { status?: number; body?: unknown; headers?: Record<string, string> }>;
  env: Record<string, string>;
  /* When true, a request no route matches goes out over the real network
     instead of answering 404. */
  passthrough?: boolean;
}

export interface RunLine {
  kind: "log" | "error" | "request" | "done";
  text: string;
}

export function parseRunConfig(json: string): RunConfig | string {
  try {
    const raw = JSON.parse(json) as Partial<RunConfig>;
    return {
      routes: raw.routes ?? {},
      env: raw.env ?? {},
      passthrough: raw.passthrough ?? false,
    };
  } catch (err) {
    return `mocks.json: ${String(err)}`;
  }
}

/* Match "METHOD /path" keys against a request; the first exact
   method+pathname key wins. */
export function matchRoute(
  routes: RunConfig["routes"],
  method: string,
  url: string,
): { status: number; body: unknown; headers: Record<string, string> } | null {
  let pathname: string;
  try {
    pathname = new URL(url, "http://mock.local").pathname;
  } catch {
    pathname = url;
  }
  const hit = routes[`${method.toUpperCase()} ${pathname}`];
  if (!hit) return null;
  return { status: hit.status ?? 200, body: hit.body ?? {}, headers: hit.headers ?? {} };
}

/* The harness module bundled in front of the user's code. It runs inside the
   worker: console and fetch are patched before any SDK code loads. */
export function harnessSource(config: RunConfig): string {
  return `
const config = ${JSON.stringify(config)};
const post = (kind, text) => self.postMessage({ kind, text });
const show = (v) => {
  if (typeof v === "string") return v;
  try { return JSON.stringify(v, null, 1); } catch { return String(v); }
};
for (const level of ["log", "info", "warn", "error"]) {
  const kind = level === "error" ? "error" : "log";
  console[level] = (...args) => post(kind, args.map(show).join(" "));
}
globalThis.process = { env: config.env };
const realFetch = globalThis.fetch.bind(globalThis);
globalThis.fetch = async (input, init) => {
  const url = typeof input === "string" ? input : input.url;
  const method = (init && init.method) || (typeof input === "object" && input.method) || "GET";
  const pathname = new URL(url, "http://mock.local").pathname;
  const hit = config.routes[method.toUpperCase() + " " + pathname];
  if (!hit && config.passthrough) {
    post("request", method.toUpperCase() + " " + url + " (live)");
    return realFetch(input, init);
  }
  post("request", method.toUpperCase() + " " + url + (hit ? "" : " (no mock, 404)"));
  const status = hit ? (hit.status ?? 200) : 404;
  const body = hit ? (hit.body ?? {}) : { error: "no mock for " + method + " " + pathname };
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...(hit && hit.headers ? hit.headers : {}) },
  });
};
self.addEventListener("unhandledrejection", (e) => post("error", show(e.reason)));
export {};
`;
}

const ENTRY = `
import "playground:harness";
import("playground:user").then(
  () => self.postMessage({ kind: "done", text: "" }),
  (err) => {
    self.postMessage({ kind: "error", text: err instanceof Error ? (err.stack ?? err.message) : String(err) });
    self.postMessage({ kind: "done", text: "" });
  },
);
`;

interface Esbuild {
  initialize(options: { wasmURL: string }): Promise<void>;
  build(options: object): Promise<{ outputFiles: { text: string }[] }>;
}

let esbuildReady: Promise<Esbuild> | null = null;

async function loadEsbuild(): Promise<Esbuild> {
  if (!esbuildReady) {
    esbuildReady = (async () => {
      const esbuild = (await import("esbuild-wasm")) as unknown as Esbuild;
      const wasmUrl = (await import("esbuild-wasm/esbuild.wasm?url")).default;
      await esbuild.initialize({ wasmURL: wasmUrl });
      return esbuild;
    })();
  }
  return esbuildReady;
}

/* Virtual filesystem for the bundler: the generated TS files keep their
   relative layout (minus the "typescript/" target prefix), the runtime
   sources live under runtime/, and "sdk" resolves to the module barrel. */
function virtualFiles(
  files: GeneratedFile[],
  runtime: Record<string, string>,
): { fs: Map<string, string>; sdkEntry: string } {
  const fs = new Map<string, string>();
  let sdkEntry = "";
  for (const file of files) {
    const path = file.path.replace(/^typescript\//, "");
    if (path === "package.json") continue;
    fs.set(path, file.text);
    if (path.endsWith("/index.ts") && !sdkEntry) sdkEntry = path;
  }
  for (const [name, text] of Object.entries(runtime)) {
    fs.set(`runtime/${name}`, text);
  }
  if (!sdkEntry) sdkEntry = files.length > 0 ? "index.ts" : "";
  return { fs, sdkEntry };
}

function resolveVirtual(fs: Map<string, string>, spec: string): string | null {
  const candidates = [spec, `${spec}.ts`, `${spec}/index.ts`];
  for (const c of candidates) if (fs.has(c)) return c;
  return null;
}

export async function bundleRun(options: {
  userCode: string;
  sdkFiles: GeneratedFile[];
  runtimeSources: Record<string, string>;
  config: RunConfig;
}): Promise<string> {
  const esbuild = await loadEsbuild();
  const { fs, sdkEntry } = virtualFiles(options.sdkFiles, options.runtimeSources);
  const plugin = {
    name: "playground-virtual",
    setup(build: {
      onResolve(filter: { filter: RegExp }, cb: (args: { path: string; importer: string }) => unknown): void;
      onLoad(filter: { filter: RegExp; namespace: string }, cb: (args: { path: string }) => unknown): void;
    }) {
      build.onResolve({ filter: /.*/ }, (args) => {
        if (args.path === "playground:entry") return { path: "playground:entry", namespace: "v" };
        if (args.path === "playground:harness") return { path: "playground:harness", namespace: "v" };
        if (args.path === "playground:user") return { path: "playground:user", namespace: "v" };
        if (args.path === "sdk") return { path: sdkEntry, namespace: "v" };
        if (args.path.startsWith("sdk/")) {
          const hit = resolveVirtual(fs, args.path.slice(4));
          if (hit) return { path: hit, namespace: "v" };
        }
        if (args.path === "@tono/http-runtime-ts") return { path: "runtime/index.ts", namespace: "v" };
        if (args.path.startsWith(".")) {
          const base = args.importer.includes("/")
            ? args.importer.slice(0, args.importer.lastIndexOf("/"))
            : "";
          const joined = new URL(args.path, `file:///${base}/`).pathname.replace(/^\//, "");
          const hit = resolveVirtual(fs, joined);
          if (hit) return { path: hit, namespace: "v" };
          return { errors: [{ text: `unresolved import ${args.path} from ${args.importer}` }] };
        }
        return { errors: [{ text: `unresolved import ${args.path}` }] };
      });
      build.onLoad({ filter: /.*/, namespace: "v" }, (args) => {
        if (args.path === "playground:entry") return { contents: ENTRY, loader: "ts" };
        if (args.path === "playground:harness")
          return { contents: harnessSource(options.config), loader: "ts" };
        if (args.path === "playground:user") return { contents: options.userCode, loader: "ts" };
        const contents = fs.get(args.path);
        if (contents === undefined) return { errors: [{ text: `missing virtual file ${args.path}` }] };
        return { contents, loader: "ts" };
      });
    },
  };
  const result = await esbuild.build({
    entryPoints: ["playground:entry"],
    bundle: true,
    write: false,
    format: "esm",
    target: "es2022",
    plugins: [plugin],
  });
  return result.outputFiles[0].text;
}

export interface ServerCapabilities {
  version: string;
  runTargets: string[];
}

/* Present when the page is served by `tono playground`; a static host has no
   /api and the fetch fails fast. */
export async function fetchCapabilities(): Promise<ServerCapabilities | null> {
  try {
    const response = await fetch("api/capabilities");
    if (!response.ok) return null;
    return (await response.json()) as ServerCapabilities;
  } catch {
    return null;
  }
}

export async function runOnServer(payload: {
  source: string;
  target: string;
  snippet: string;
  mocks: RunConfig;
}): Promise<RunLine[]> {
  try {
    const response = await fetch("api/run", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(payload),
    });
    const data = (await response.json()) as { lines?: RunLine[]; error?: string };
    if (!response.ok) {
      return [
        { kind: "error", text: data.error ?? response.statusText },
        { kind: "done", text: "" },
      ];
    }
    return [...(data.lines ?? []), { kind: "done", text: "" }];
  } catch (err) {
    return [
      { kind: "error", text: String(err) },
      { kind: "done", text: "" },
    ];
  }
}

export function runInWorker(bundle: string, onLine: (line: RunLine) => void): { stop: () => void } {
  const blob = new Blob([bundle], { type: "text/javascript" });
  const url = URL.createObjectURL(blob);
  const worker = new Worker(url, { type: "module" });
  const finish = (): void => {
    worker.terminate();
    URL.revokeObjectURL(url);
    clearTimeout(timer);
  };
  const timer = setTimeout(() => {
    onLine({ kind: "error", text: "timed out after 10s" });
    onLine({ kind: "done", text: "" });
    finish();
  }, 10_000);
  worker.addEventListener("message", (event) => {
    const line = event.data as RunLine;
    onLine(line);
    if (line.kind === "done") finish();
  });
  worker.addEventListener("error", (event) => {
    onLine({ kind: "error", text: event.message });
    onLine({ kind: "done", text: "" });
    finish();
  });
  return { stop: finish };
}

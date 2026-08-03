/* Real TypeScript intelligence for the Run snippet: a TypeScript language
   service over an in-memory filesystem holding the generated SDK and the HTTP
   runtime sources, wired into CodeMirror. Everything loads lazily (the
   compiler is megabytes) and only when the Run panel is on TypeScript. */
import { autocompletion } from "@codemirror/autocomplete";
import type { Extension } from "@codemirror/state";
import type { GeneratedFile } from "./types";

export interface TsLang {
  extensions: Extension[];
  update(files: GeneratedFile[], runtime: Record<string, string>, moduleName: string): void;
}

/* The default TypeScript lib files, bundled as lazy raw assets so the
   language service works offline. */
const libLoaders = import.meta.glob("/node_modules/typescript/lib/lib.*.d.ts", {
  query: "?raw",
  import: "default",
});

let loading: Promise<TsLang> | null = null;

export function loadTsLang(): Promise<TsLang> {
  if (!loading) loading = create();
  return loading;
}

async function create(): Promise<TsLang> {
  const [tsModule, vfs, cmts] = await Promise.all([
    import("typescript"),
    import("@typescript/vfs"),
    import("@valtown/codemirror-ts"),
  ]);
  const ts = tsModule.default;
  const fsMap = new Map<string, string>();
  for (const [path, load] of Object.entries(libLoaders)) {
    const name = path.split("/").pop();
    if (name) fsMap.set(`/${name}`, (await load()) as string);
  }
  // The vfs treats an empty string as a missing file (readFile returning a
  // falsy value), so seeds are never "".
  fsMap.set("/main.ts", "\n");
  fsMap.set("/sdk-entry.ts", "export {};\n");
  const system = vfs.createSystem(fsMap);
  const env = vfs.createVirtualTypeScriptEnvironment(system, ["/main.ts"], ts, {
    target: ts.ScriptTarget.ES2022,
    module: ts.ModuleKind.ESNext,
    moduleResolution: ts.ModuleResolutionKind.Bundler,
    strict: true,
    noEmit: true,
    baseUrl: "/",
    /* "sdk" points at a stable alias file so the mapping survives module
       renames; the alias re-exports whatever barrel the current module has. */
    paths: {
      sdk: ["/sdk-entry.ts"],
      "@tono/http-runtime-ts": ["/runtime/index.ts"],
    },
  });

  const upsert = (path: string, text: string): void => {
    if (env.getSourceFile(path)) env.updateFile(path, text);
    else env.createFile(path, text);
  };

  return {
    extensions: [
      cmts.tsFacet.of({ env, path: "/main.ts" }),
      cmts.tsSync(),
      cmts.tsLinter(),
      autocompletion({ override: [cmts.tsAutocomplete()] }),
      cmts.tsHover(),
    ],
    update(files, runtime, moduleName) {
      for (const [name, text] of Object.entries(runtime)) {
        upsert(`/runtime/${name}`, text);
      }
      let barrel: string | null = null;
      for (const file of files) {
        const rel = file.path.replace(/^typescript\//, "");
        if (rel.endsWith("package.json")) continue;
        upsert(`/sdk/${rel}`, file.text);
        if (!barrel && rel.endsWith("index.ts")) barrel = rel.replace(/\.ts$/, "");
      }
      void moduleName;
      upsert(
        "/sdk-entry.ts",
        barrel ? `export * from "./sdk/${barrel}";\n` : "export {};\n",
      );
    },
  };
}

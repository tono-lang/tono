/* End-to-end smoke over the built compiler artifacts, run under node: the
   real frontend compiles each bundled example and the real wasm backend
   generates every target from the resulting IR. Guards the same contract the
   browser relies on, without needing a browser. */
import { strict as assert } from "node:assert";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const require = createRequire(import.meta.url);

// Under CommonJS the jsoo runtime exports onto module.exports; in the browser
// (no module object) the same artifact attaches to globalThis instead.
const shim = require(join(root, "src/generated/tono_frontend.cjs"));
const frontend = shim.tonoFrontend ?? globalThis.tonoFrontend;
assert.ok(frontend, "jsoo shim must export tonoFrontend");

const backend = await import(
  join(root, "src/generated/backend/tono_playground_backend.js")
);
await backend.default({
  module_or_path: await readFile(
    join(root, "src/generated/backend/tono_playground_backend_bg.wasm"),
  ),
});

assert.equal(
  frontend.irVersion(),
  backend.ir_version(),
  "frontend and backend must agree on the IR version",
);

// Node strips the type annotations natively (node >= 22.18), so the app's own
// example module is the single source for what must compile.
const { EXAMPLES } = await import(join(root, "src/examples.ts"));

const targets = ["ts", "rust", "go"];
// Which examples must generate for every target; the bespoke-auth one
// deliberately fails impl coverage outside TypeScript.
const fullMatrix = ["Payment methods", "HTTP client"];

for (const example of EXAMPLES) {
  const result = frontend.compile(example.source);
  const errors = result.diagnostics.filter((d) => d.severity === "error");
  assert.equal(errors.length, 0, `${example.name}: ${JSON.stringify(errors)}`);
  assert.ok(result.ir, `${example.name}: expected IR`);
  for (const target of targets) {
    let files;
    try {
      files = JSON.parse(backend.generate(String(result.ir), target)).files;
    } catch (err) {
      assert.ok(
        !fullMatrix.includes(example.name),
        `${example.name} -> ${target}: ${err}`,
      );
      continue;
    }
    assert.ok(files.length > 0, `${example.name} -> ${target}: no files`);
    for (const file of files) {
      assert.ok(file.path, "file has a path");
      assert.ok(file.text.length > 0, `${file.path}: empty text`);
    }
  }
  console.log(`ok: ${example.name}`);
}

const broken = frontend.compile("struct x { name: strin }");
assert.equal(broken.ir, null);
assert.ok(broken.diagnostics.some((d) => d.code === "TC0001"));
console.log("ok: diagnostics carry codes and spans");

const fmt = frontend.formatSource("pub   enum   status{active}");
assert.equal(fmt.formatted, "pub enum status {\n  active\n}\n");
console.log("ok: format");

// IDE surface: outline, naming index, hover, completion, definition.
{
  const src = EXAMPLES[0].source;
  const decls = frontend.decls(src);
  assert.ok(decls.some((d) => d.name === "payment_method" && d.kind === "union"));
  const ir = String(frontend.compile(src).ir);
  for (const target of targets) {
    const syms = JSON.parse(backend.symbols(ir, target));
    const pm = syms.find((s) => s.id === "playground#payment_method");
    assert.ok(pm && pm.ident.length > 0, `${target}: payment_method has an ident`);
  }
  const tsSyms = JSON.parse(backend.symbols(ir, "ts"));
  assert.equal(tsSyms.find((s) => s.id === "playground#payment_method")?.ident, "PaymentMethod");

  const hover = frontend.hoverAt("pub struct card { last4: string }", 0, 12);
  assert.ok(hover && hover.contents.includes("struct card"));
  const completions = frontend.completionsAt("pub struct x { f: ", 0, 18);
  assert.ok(Array.from(completions).some((c) => c.label === "string"));
  const def = frontend.definitionAt(
    "pub struct card { last4: string }\npub op show(card): string\n",
    1,
    12,
  );
  assert.ok(def && def.start.line === 0, "card reference resolves to line 0");
  console.log("ok: ide surface (decls, symbols, hover, completion, definition)");
}

console.log("smoke: all good");

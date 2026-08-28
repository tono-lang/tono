// The TypeScript extractor: the exported symbols of one package, read
// through the TypeScript compiler API, printed as the index's neutral JSON.
// Run by `tono index` with node from a scratch directory inside the consumer
// tree, so the package resolves the way the generated SDK's imports do.
//
// The file is CommonJS by extension (`.cjs`): the scratch directory it runs
// from sits under the SDK's own `package.json`, which declares
// `"type": "module"`, and node would read a `.js` there as an ES module.
//
// The compiler API is what resolves re-exports (`export * from`, `export {
// a as b } from`, `export =`), overloads, and the difference between a
// class's static and instance members: this script only walks what the
// checker hands back. The API itself is not bundled: it is loaded from the
// first candidate path given on the command line (the `typescript` package
// beside the library, or the `typescript-api` alias), because the native
// compiler shipped as `typescript` 7 has no scripting API.
//
// Usage: node extract.cjs <root> <package> <api candidate>...
"use strict";

const path = require("path");

function emit(report) {
  if (!report.symbols) report.symbols = [];
  process.stdout.write(JSON.stringify(report) + "\n");
}

function skip(reason) {
  emit({ skipped: reason });
  process.exit(0);
}

function loadApi(candidates) {
  for (const candidate of candidates) {
    try {
      const ts = require(candidate);
      if (ts && typeof ts.createProgram === "function") return ts;
    } catch (e) {
      // A candidate that does not load is not the API; the next may be.
    }
  }
  return null;
}

const [root, pkg, ...candidates] = process.argv.slice(2);
if (!root || !pkg) {
  process.stderr.write("usage: extract.cjs <root> <package> <api candidate>...\n");
  process.exit(2);
}
const ts = loadApi(candidates);
if (!ts) {
  skip(
    "no TypeScript compiler API beside the library (the typescript package found has none; install typescript 5 or the typescript-api alias)"
  );
}

const options = {
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  noEmit: true,
  skipLibCheck: true,
  types: [],
};
const host = ts.createCompilerHost(options);
const resolved = ts.resolveModuleName(pkg, path.join(root, "probe.ts"), options, host);
if (!resolved.resolvedModule) {
  skip(`package ${pkg} does not resolve from ${root} (is it installed?)`);
}
const entry = resolved.resolvedModule.resolvedFileName;
const program = ts.createProgram([entry], options, host);
const checker = program.getTypeChecker();
const sourceFile = program.getSourceFile(entry);
const moduleSymbol = sourceFile && checker.getSymbolAtLocation(sourceFile);
if (!moduleSymbol) {
  skip(`${entry} declares no module exports`);
}

const FLAGS = ts.TypeFormatFlags.NoTruncation;
const F = ts.SymbolFlags;

function resolve(symbol) {
  return symbol.flags & F.Alias ? checker.getAliasedSymbol(symbol) : symbol;
}

function declOf(symbol) {
  return symbol.valueDeclaration || (symbol.declarations && symbol.declarations[0]);
}

function kindOf(symbol) {
  const flags = symbol.flags;
  if (flags & F.Class) return "class";
  if (flags & F.Function) return "function";
  if (flags & F.Interface) return "interface";
  if (flags & F.TypeAlias) return "type";
  if (flags & F.Enum) return "enum";
  if (flags & F.Module) return "namespace";
  if (flags & (F.Variable | F.Property)) return "const";
  return null;
}

function docOf(symbol) {
  const text = ts.displayPartsToString(symbol.getDocumentationComment(checker)).trim();
  const paragraph = text.split(/\n\s*\n/)[0].replace(/\s+/g, " ");
  return paragraph.length > 300 ? paragraph.slice(0, 297) + "..." : paragraph;
}

function callSignatures(type) {
  return type.getCallSignatures().map((s) => checker.signatureToString(s, undefined, FLAGS));
}

function typeOf(symbol) {
  return checker.getTypeOfSymbolAtLocation(symbol, declOf(symbol) || sourceFile);
}

function isHidden(symbol) {
  const name = symbol.name;
  if (name.startsWith("#") || name.startsWith("__")) return true;
  const decl = declOf(symbol);
  return !!decl && !!(ts.getCombinedModifierFlags(decl) & ts.ModifierFlags.NonPublicAccessibilityModifier);
}

// A property of a type as an index member: a method when its type is
// callable (every overload kept), a field otherwise.
function memberOf(property, isStatic) {
  const type = typeOf(property);
  const calls = callSignatures(type);
  return {
    name: property.name,
    kind: calls.length ? "method" : "field",
    static: isStatic,
    signatures: calls.length ? calls : [checker.typeToString(type, undefined, FLAGS)],
  };
}

function membersOfType(type, isStatic) {
  return type
    .getProperties()
    .filter((p) => !isHidden(p) && p.name !== "prototype")
    .map((p) => memberOf(p, isStatic));
}

function namespaceMembers(symbol, prefix) {
  const members = [];
  for (const exported of checker.getExportsOfModule(symbol)) {
    const target = resolve(exported);
    const kind = kindOf(target);
    const name = prefix + exported.name;
    if (kind === "namespace") {
      members.push(...namespaceMembers(target, name + "."));
      continue;
    }
    if (kind === "function" || kind === "const") {
      const calls = callSignatures(typeOf(target));
      members.push({
        name,
        kind: calls.length ? "function" : "const",
        static: true,
        signatures: calls.length ? calls : [checker.typeToString(typeOf(target), undefined, FLAGS)],
      });
    } else if (kind) {
      members.push({ name, kind: "type", static: true, signatures: [] });
    }
  }
  return members;
}

function symbolOf(name, target) {
  const kind = kindOf(target);
  if (!kind) return null;
  const symbol = { name, kind, signatures: [], doc: docOf(target), members: [] };
  switch (kind) {
    case "function": {
      symbol.signatures = callSignatures(typeOf(target));
      break;
    }
    case "const": {
      const type = typeOf(target);
      const calls = callSignatures(type);
      if (calls.length) {
        symbol.kind = "function";
        symbol.signatures = calls;
      } else {
        symbol.signatures = [checker.typeToString(type, undefined, FLAGS)];
      }
      break;
    }
    case "class": {
      const constructor = typeOf(target);
      symbol.signatures = constructor
        .getConstructSignatures()
        .map((s) => checker.signatureToString(s, undefined, FLAGS));
      symbol.members = membersOfType(constructor, true).concat(
        membersOfType(checker.getDeclaredTypeOfSymbol(target), false)
      );
      break;
    }
    case "interface": {
      symbol.members = membersOfType(checker.getDeclaredTypeOfSymbol(target), false);
      break;
    }
    case "type": {
      // An alias of an object type lists its members; any other alias (a
      // union, a primitive) shows what it stands for, as written, because
      // the apparent properties of a union are the primitives' own.
      const declared = checker.getDeclaredTypeOfSymbol(target);
      const decl = declOf(target);
      if (declared.flags & ts.TypeFlags.Object) {
        symbol.members = membersOfType(declared, false);
      } else if (decl && decl.type) {
        symbol.signatures = [decl.type.getText()];
      } else {
        symbol.signatures = [checker.typeToString(declared, undefined, FLAGS)];
      }
      break;
    }
    case "enum": {
      if (target.exports) {
        target.exports.forEach((member) => {
          symbol.members.push({ name: member.name, kind: "const", static: true, signatures: [] });
        });
      }
      break;
    }
    case "namespace": {
      symbol.members = namespaceMembers(target, "");
      break;
    }
  }
  return symbol;
}

const symbols = [];
const notes = [];
for (const exported of checker.getExportsOfModule(moduleSymbol)) {
  const symbol = symbolOf(exported.name, resolve(exported));
  if (symbol) symbols.push(symbol);
}
// `export = X` hands the module's exports to X's own members; X itself is
// what a spelling names, so it is listed under the package's last segment.
const assigned = moduleSymbol.exports && moduleSymbol.exports.get("export=");
if (assigned) {
  const target = resolve(assigned);
  const name = pkg.split("/").pop().replace(/[^A-Za-z0-9_$]/g, "_");
  if (!symbols.some((s) => s.name === name)) {
    const symbol = symbolOf(name, target);
    if (symbol) {
      symbols.push(symbol);
      notes.push(`the package uses export =; its value is listed as ${name}`);
    }
  }
}
emit({ symbols, note: notes.length ? notes.join("; ") : undefined });

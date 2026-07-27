#!/usr/bin/env bash
# The bespoke gate for `ext impl`: an operation implemented in two languages is
# only trustworthy if something proves the implementations agree. This generates
# the hybrid example's SDK for Go and TypeScript, drops each language's bespoke
# sources in, and runs the same conformance vectors through both generated
# clients. The two outputs must match each other (differential) and the
# expectations the vectors declare (golden).
#
# Everything happens in a throwaway directory, so nothing leaks into the repo.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
example="$root/examples/hybrid-notes"
vectors=("$example/vectors/save_note.json" "$example/vectors/archive_note.json")
go_module="example.com/notes"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [ ! -x "$frontend" ]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe)
fi
tono="$root/target/debug/tono"
if [ ! -x "$tono" ]; then
    (cd "$root" && cargo build --quiet -p tono)
fi
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
if [ ! -x "$tsc" ]; then
    echo "the TypeScript toolchain is not installed; run 'npm ci' in backend/codegen-tests/typescript" >&2
    exit 1
fi

echo "generating..."
"$frontend" compile "$example/notes.tono" --module notes >"$work/ir.json"
"$tono" gen --target go,typescript --out "$work/sdk" --go-module "$go_module" "$work/ir.json"

echo "go..."
mkdir -p "$work/go/conformance"
cp -R "$work"/sdk/go/. "$work/go/"
# The bound symbol is called unqualified from inside the generated package, so
# the bespoke file is dropped into the module's package directory rather than
# imported.
cp "$example/ext/go/notes.go" "$work/go/notes/bespoke.go"
cp "$example/conformance/go/main.go" "$work/go/conformance/main.go"
(cd "$work/go" && go mod init "$go_module" >/dev/null 2>&1 \
    && go mod edit -require=github.com/tono-lang/tono/runtimes/http-go@v0.0.0 \
    && go mod edit -replace=github.com/tono-lang/tono/runtimes/http-go="$root/runtimes/http-go" \
    && go mod edit -require=github.com/tono-lang/tono/runtimes/ext-go@v0.0.0 \
    && go mod edit -replace=github.com/tono-lang/tono/runtimes/ext-go="$root/runtimes/ext-go" \
    && go mod tidy >/dev/null \
    && go build ./...)
(cd "$work/go" && NOTES_TOKEN=t0 go run ./conformance "${vectors[@]}") >"$work/go.json"

echo "typescript..."
# The runtimes are TypeScript sources, so they are compiled into the throwaway
# node_modules once: that makes both tsc and node resolve them the way a
# consumer's installed package would, with no path mapping to keep in sync.
compile_runtime() {
    local name="$1" src="$2" dest="$work/ts/node_modules/@tono/$1"
    mkdir -p "$dest"
    "$tsc" "$src"/*.ts --outDir "$dest" --module CommonJS \
        --target ES2020 --lib ES2020,DOM --declaration --skipLibCheck --strict
    printf '{"name":"@tono/%s","main":"index.js","types":"index.d.ts"}\n' "$name" >"$dest/package.json"
}
mkdir -p "$work/ts/conformance/ts"
cp -R "$work"/sdk/typescript/. "$work/ts/"
cp -R "$example/ext" "$work/ts/ext"
cp "$example/conformance/ts/main.ts" "$example/conformance/ts/node.d.ts" "$work/ts/conformance/ts/"
compile_runtime http-runtime-ts "$root/runtimes/http-ts/src"
compile_runtime ext-runtime-ts "$root/runtimes/ext-ts/src"
cat >"$work/ts/tsconfig.json" <<'EOF'
{
  "compilerOptions": {
    "strict": true,
    "target": "ES2020",
    "module": "CommonJS",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true,
    "types": [],
    "outDir": "js",
    "rootDir": "."
  },
  "include": ["**/*.ts"],
  "exclude": ["node_modules", "js"]
}
EOF
(cd "$work/ts" && "$tsc" -p tsconfig.json)
# node resolves the runtime packages out of the same node_modules tsc used.
(cd "$work/ts" && NOTES_TOKEN=t0 node js/conformance/ts/main.js "${vectors[@]}") >"$work/ts.json"

echo "comparing..."
python3 - "$work/go.json" "$work/ts.json" "${vectors[@]}" <<'PY'
import json, sys

go_path, ts_path, *vector_paths = sys.argv[1:]
read = lambda p: json.load(open(p))
go, ts = read(go_path), read(ts_path)

expected = []
for path in vector_paths:
    for case in read(path)["cases"]:
        expected.append({**case["expect"], "name": case["name"]})

def canon(x):
    return json.dumps(x, sort_keys=True)

failures = []
if len(go) != len(ts) or len(go) != len(expected):
    failures.append(f"case counts differ: go={len(go)} ts={len(ts)} vectors={len(expected)}")
else:
    for g, t, e in zip(go, ts, expected):
        if canon(g) != canon(t):
            failures.append(
                f"{g['name']}: the implementations disagree\n  go: {canon(g)}\n  ts: {canon(t)}"
            )
        elif canon(g) != canon(e):
            failures.append(
                f"{g['name']}: both agree but the vector expects otherwise\n"
                f"  got:      {canon(g)}\n  expected: {canon(e)}"
            )

if failures:
    print("\n".join(failures), file=sys.stderr)
    sys.exit(1)
print(f"{len(expected)} conformance cases agree across go and typescript")
PY

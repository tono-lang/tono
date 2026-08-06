#!/usr/bin/env bash
# The bespoke gate for `ext impl`: an operation implemented in two languages is
# only trustworthy if something proves the implementations agree. The hybrid
# example declares that proof as `test` blocks in the spec itself, and the
# generator emits them as native tests beside each client. This script
# generates the example's SDK for Go and TypeScript, drops each language's
# bespoke sources in, and runs the generated tests in both: the hermetic suite
# against its declared stubs, and the live suite against the real
# implementations, which for this example are the deterministic in-repo
# stores. One body of declared cases, executed in every language, is the
# differential check.
#
# The bearer example's generated Vitest suite runs here too: it asserts the
# Authorization header the TypeScript client_init hook writes.
#
# Everything happens in a throwaway directory, so nothing leaks into the repo.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
example="$root/examples/hybrid-notes"
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
vitest="$root/backend/codegen-tests/typescript/node_modules/.bin/vitest"
if [ ! -x "$vitest" ]; then
    echo "vitest is not installed; run 'npm ci' in backend/codegen-tests/typescript" >&2
    exit 1
fi

echo "generating..."
"$frontend" compile "$example/notes.tono" --module notes >"$work/ir.json"
"$tono" gen --target go,typescript --out "$work/sdk" --go-module "$go_module" "$work/ir.json"

echo "go..."
mkdir -p "$work/go"
cp -R "$work"/sdk/go/. "$work/go/"
# The bound symbol is called unqualified from inside the generated package, so
# the bespoke file is dropped into the module's package directory rather than
# imported.
cp "$example/ext/go/notes.go" "$work/go/notes/bespoke.go"
(cd "$work/go" && go mod init "$go_module" >/dev/null 2>&1 \
    && go mod edit -require=github.com/tono-lang/tono/runtimes/ext-go@v0.0.0 \
    && go mod edit -replace=github.com/tono-lang/tono/runtimes/ext-go="$root/runtimes/ext-go" \
    && go mod tidy >/dev/null \
    && go build ./...)
echo "go test (hermetic)..."
(cd "$work/go" && go test ./...)
echo "go test (live)..."
(cd "$work/go" && NOTES_TOKEN=t0 go test -tags live ./...)

echo "typescript..."
# The ext runtime is TypeScript source, so it is compiled into the throwaway
# node_modules once: that makes both tsc and node resolve it the way a
# consumer's installed package would, with no path mapping to keep in sync.
compile_runtime() {
    local name="$1" src="$2" dest="$work/ts/node_modules/@tono/$1"
    mkdir -p "$dest"
    "$tsc" "$src"/*.ts --outDir "$dest" --module CommonJS \
        --target ES2020 --lib ES2020,DOM --declaration --skipLibCheck --strict
    printf '{"name":"@tono/%s","main":"index.js","types":"index.d.ts"}\n' "$name" >"$dest/package.json"
}
mkdir -p "$work/ts"
cp -R "$work"/sdk/typescript/. "$work/ts/"
cp -R "$example/ext" "$work/ts/ext"
compile_runtime ext-runtime-ts "$root/runtimes/ext-ts/src"
echo "vitest (hermetic + live)..."
(cd "$work/ts" && TONO_LIVE_TESTS=1 NOTES_TOKEN=t0 "$vitest" run --root .)

echo "auth-bearer vitest..."
# The bearer example's generated test asserts the Authorization header the
# TypeScript client_init hook writes; running it here reuses the runtimes this
# script already compiled into node_modules.
"$frontend" compile "$root/examples/auth-bearer/auth.tono" --module auth >"$work/auth-ir.json"
"$tono" gen --target typescript --out "$work/auth-sdk" "$work/auth-ir.json"
mkdir -p "$work/auth-ts/node_modules"
cp -R "$work/auth-sdk/typescript/." "$work/auth-ts/"
cp -R "$root/examples/auth-bearer/ext" "$work/auth-ts/ext"
cp -R "$work/ts/node_modules/@tono" "$work/auth-ts/node_modules/@tono"
(cd "$work/auth-ts" && "$vitest" run --root .)

echo "generated tests pass in go and typescript"

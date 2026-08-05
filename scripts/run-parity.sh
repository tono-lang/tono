#!/usr/bin/env bash
# Runs the cross-runtime parity vectors (runtimes/parity/vectors.json) against
# a real generated SDK: compiles runtimes/parity/spec.tono, generates the SDK
# into a throwaway directory, drops the hand-written harness in next to it,
# and runs that target's native test suite.
#
# Only TypeScript is repointed against the generated SDK so far; Go and Rust
# still exercise their runtime packages directly (see runtimes/parity/README.md).
# This script is structured so those two slot in as additional cases later,
# without a rewrite.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
lang="${1:-typescript}"

if [ "$lang" != "typescript" ]; then
    echo "run-parity.sh: unsupported target '$lang' (only 'typescript' is repointed so far)" >&2
    exit 1
fi

echo "building the frontend and backend CLIs..."
frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [ ! -x "$frontend" ]; then
    opam exec -- dune build frontend/bin/tono_frontend.exe
fi
cargo build --quiet --bin tono
tono="$root/target/debug/tono"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "compiling runtimes/parity/spec.tono..."
"$frontend" compile "$root/runtimes/parity/spec.tono" --module parity >"$work/ir.json"

echo "generating the TypeScript SDK..."
# Best-effort formatting of the throwaway SDK: the pinned prettier from the
# codegen test toolchain, if it happens to be installed (as scripts/regen-example.sh
# uses it). Unlike that script this is not a correctness requirement here (this
# output is never committed), so a missing toolchain only means an unformatted
# warning, not a failure.
if [ -d "$root/backend/codegen-tests/typescript/node_modules/.bin" ]; then
    export PATH="$root/backend/codegen-tests/typescript/node_modules/.bin:$PATH"
fi
"$tono" gen --target typescript --out "$work/sdk" <"$work/ir.json"

echo "dropping the parity harness into the generated SDK..."
cp "$root/runtimes/parity/typescript/parity.test.ts" "$work/sdk/typescript/"
cp "$root/runtimes/parity/vectors.json" "$work/sdk/typescript/"

echo "running the parity suite against the generated SDK..."
# Reuses runtimes/http-ts's own Vitest install rather than provisioning a
# second toolchain for a throwaway package; --root points discovery at the
# generated tree instead of at runtimes/http-ts's own tests.
if [ ! -d "$root/runtimes/http-ts/node_modules" ]; then
    (cd "$root/runtimes/http-ts" && npm ci --ignore-scripts)
fi
(cd "$root/runtimes/http-ts" && node node_modules/.bin/vitest run --root "$work/sdk/typescript")

echo "parity suite passed against the generated $lang SDK"

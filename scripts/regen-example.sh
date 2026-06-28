#!/usr/bin/env bash
# Regenerate the checked-in example SDK end to end:
#   payments.tono -> (frontend) -> ir.json -> (tono gen) -> sdk/{rust,go,typescript}
#
# CI runs this and fails if the working tree changes (the drift guard), so the
# committed SDK always matches what the current compiler produces. Run it after
# touching the frontend, the engine, or the example source, and commit the diff.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
example="examples/payments"

# The TypeScript formatter is the pinned prettier from the codegen test toolchain
# (a fixed version, also used in CI), so formatting is deterministic.
export PATH="$root/backend/codegen-tests/typescript/node_modules/.bin:$PATH"

echo "building the frontend and backend CLIs..."
opam exec -- dune build frontend/bin/main.exe
cargo build --quiet --bin tono

frontend="$root/_build/default/frontend/bin/main.exe"
tono="$root/target/debug/tono"

echo "compiling $example/payments.tono to IR..."
"$frontend" compile "$example/payments.tono" --module payments >"$example/ir.json"

echo "generating the SDK..."
"$tono" gen --target rust,go,typescript --out "$example/sdk" <"$example/ir.json"

echo "done. SDK written to $example/sdk/"

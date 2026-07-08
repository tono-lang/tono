#!/usr/bin/env bash
# Regenerate the checked-in example SDK end to end:
#   src/**.tono -> (frontend) -> ir.json -> (tono gen) -> sdk/{rust,go,typescript}
#
# The example is a two-module project (payments.common, payments.charges) so it
# exercises imports, cross-module visibility, and the per-language sub-package
# mapping. CI runs this and fails if the working tree changes (the drift guard),
# so the committed SDK always matches what the current compiler produces. Run it
# after touching the frontend, the engine, or the example source, and commit the
# diff.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
example="examples/payments"

# The SDK's Go module path, prefixed onto cross-package imports (Go has no
# relative imports).
go_module="example.com/sdk"

# The TypeScript formatter is the pinned prettier from the codegen test toolchain
# (a fixed version, also used in CI), so formatting is deterministic.
export PATH="$root/backend/codegen-tests/typescript/node_modules/.bin:$PATH"

echo "building the frontend and backend CLIs..."
opam exec -- dune build frontend/bin/tono_frontend.exe
cargo build --quiet --bin tono

frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
tono="$root/target/debug/tono"

echo "compiling $example/src to IR..."
"$frontend" compile-dir "$example/src" >"$example/ir.json"

echo "generating the SDK..."
# A fresh directory each run so a renamed or removed module leaves no stale file.
rm -rf "$example/sdk"
"$tono" gen --target rust,go,typescript --out "$example/sdk" \
  --go-module "$go_module" <"$example/ir.json"

echo "done. SDK written to $example/sdk/"

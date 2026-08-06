#!/usr/bin/env bash
# Build the two compiler halves the web app embeds, straight from this repo:
#   1. OCaml frontend -> playground/src/generated/tono_frontend.js  (js_of_ocaml)
#   2. Rust backend   -> playground/src/generated/backend/          (wasm-bindgen)
# Requires: opam env with dune, yojson, js_of_ocaml, js_of_ocaml-ppx, lsp,
# jsonrpc; rustup with the wasm32-unknown-unknown target; node deps installed.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
pg="$root/playground"

echo "build: js_of_ocaml frontend"
(cd "$root" && dune build --profile release playground/shim/shim.bc.js)
mkdir -p "$pg/src/generated"
rm -f "$pg/src/generated/tono_frontend.js" "$pg/src/generated/tono_frontend.cjs"
cp "$root/_build/default/playground/shim/shim.bc.js" "$pg/src/generated/tono_frontend.js"
# The same artifact under a .cjs name: node loads it as CommonJS there, which
# the smoke test needs (the jsoo node runtime calls require at startup).
cp "$root/_build/default/playground/shim/shim.bc.js" "$pg/src/generated/tono_frontend.cjs"
chmod 644 "$pg/src/generated/tono_frontend.js" "$pg/src/generated/tono_frontend.cjs"
printf 'export {};\n' > "$pg/src/generated/tono_frontend.d.ts"

echo "build: wasm backend"
# CI installs wasm-pack as a prebuilt tool (npm ci runs with --ignore-scripts,
# which skips the npm package's binary download); locally the npm dep works.
if command -v wasm-pack >/dev/null 2>&1; then
  wasm_pack=wasm-pack
else
  wasm_pack="$pg/node_modules/.bin/wasm-pack"
fi
(cd "$pg/backend-wasm" &&
  "$wasm_pack" build --target web --release \
    --out-dir "$pg/src/generated/backend" --no-pack)

echo "build: done"

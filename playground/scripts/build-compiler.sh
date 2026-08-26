#!/usr/bin/env bash
# Build the artifacts the web app embeds, straight from this repo:
#   1. OCaml frontend  -> playground/src/generated/tono_frontend.js  (js_of_ocaml)
#   2. Rust backend    -> playground/src/generated/backend/          (wasm-bindgen)
#   3. tono grammar    -> playground/src/generated/tree-sitter-tono.wasm (highlighting)
# Requires: opam env with dune, yojson, js_of_ocaml, js_of_ocaml-ppx, lsp,
# jsonrpc; rustup with the wasm32-unknown-unknown target; node deps installed;
# the tree-sitter CLI; git.
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

echo "build: tono grammar (tree-sitter-tono, wasm)"
# The grammar lives in its own repo, pinned to a rev in cli/Cargo.toml (the same
# pin the CLI's preview panes build against), so a clone-and-build here always
# matches what the CLI ships instead of drifting from a manually copied asset.
ts_rev="$(sed -n 's/.*tree-sitter-tono = { git = "\([^"]*\)", rev = "\([a-f0-9]*\)".*/\1 \2/p' "$root/cli/Cargo.toml")"
ts_repo="${ts_rev%% *}"
ts_rev="${ts_rev##* }"
if [ -z "$ts_repo" ] || [ -z "$ts_rev" ]; then
  echo "could not find the tree-sitter-tono git/rev pin in cli/Cargo.toml" >&2
  exit 1
fi
ts_src="$(mktemp -d)"
trap 'rm -rf "$ts_src"' EXIT
git clone --quiet "$ts_repo" "$ts_src"
git -C "$ts_src" checkout --quiet "$ts_rev"
(cd "$ts_src" && tree-sitter generate && tree-sitter build --wasm)
cp "$ts_src/tree-sitter-tono.wasm" "$pg/src/generated/tree-sitter-tono.wasm"
cp "$ts_src/queries/highlights.scm" "$pg/src/generated/tono-highlights.scm"

echo "build: done"

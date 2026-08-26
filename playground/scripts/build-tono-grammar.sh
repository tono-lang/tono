#!/usr/bin/env bash
# Build tree-sitter-tono to wasm -> playground/src/generated/tree-sitter-tono.wasm
# and copy its highlights query alongside it. The grammar lives in its own
# repo, pinned to a rev in cli/Cargo.toml (the same pin the CLI's preview
# panes build against), so a clone-and-build here always matches what the CLI
# ships instead of drifting from a manually copied asset.
# Requires: the tree-sitter CLI; git.
#
# A separate script from build-compiler.sh's other two steps because the
# Sonar coverage job needs this one alone (its playground tests exercise the
# highlighter) without the OCaml/Rust toolchains the other two steps need.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
pg="$root/playground"

echo "build: tono grammar (tree-sitter-tono, wasm)"
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
mkdir -p "$pg/src/generated"
cp "$ts_src/tree-sitter-tono.wasm" "$pg/src/generated/tree-sitter-tono.wasm"
cp "$ts_src/queries/highlights.scm" "$pg/src/generated/tono-highlights.scm"

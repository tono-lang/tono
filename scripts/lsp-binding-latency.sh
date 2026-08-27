#!/usr/bin/env bash
# Save-to-diagnostic latency of the editor's binding check, measured from the
# client's side: the language server is driven like an editor on a bench
# probe whose Go binding is wrong, the file is saved N times with one
# language pair dirtied each time (the common case: one block edited), and
# the time from the save to the published verdict is sampled. The libraries
# resolve from the same consumer trees the FFI bench uses.
#
# usage: scripts/lsp-binding-latency.sh [saves]   (default 20)
set -euo pipefail

cd "$(dirname "$0")/.." || exit 1
root="$PWD"
bench="$root/examples/mathkit"
saves="${1:-20}"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
lsp="$root/_build/default/lsp/tono_lsp.exe"
driver="$root/_build/default/lsp/test/lsp_check_driver.exe"
if [[ ! -x "$frontend" || ! -x "$lsp" || ! -x "$driver" ]]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe lsp/tono_lsp.exe lsp/test/lsp_check_driver.exe)
fi
tono="$root/target/debug/tono"
if [[ ! -x "$tono" ]]; then
    (cd "$root" && cargo build -p tono-cli --quiet)
fi
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
if [[ ! -x "$tsc" ]]; then
    echo "tsc is not installed; run 'npm ci' in backend/codegen-tests/typescript" >&2
    exit 1
fi
export PATH="$root/backend/codegen-tests/typescript/node_modules/.bin:$PATH"
export TONO_FRONTEND="$frontend"

check_go="$work/check-go"
mkdir -p "$check_go"
cat >"$check_go/go.mod" <<GOMOD
module example.com/check

go 1.21

require tono-ext-fixture/mathkit v0.0.0
replace tono-ext-fixture/mathkit => $bench/ext/go
GOMOD
check_ts="$work/check-ts"
mkdir -p "$check_ts/node_modules/@tono-ext-fixture"
cp -R "$bench/ext/ts" "$check_ts/node_modules/@tono-ext-fixture/mathkit"

proj="$work/project"
mkdir -p "$proj"
cp "$bench/probes/21-go-generated-type-wrong-binding.tono" "$proj/mathkit.tono"
cat >"$proj/tono.toml" <<TOML
[project]
name = "mathkit-sdk"

[target.go]
enabled = true
package = "example.com/check"
out = "$check_go"

[target.typescript]
enabled = true
package = "mathkit-sdk"
out = "$check_ts"

[ext.mathkit]
rust = "0.0.0"
go = "v0.0.0"
ts = "0.0.0"
TOML

# Warm the Go build cache the way a developer's machine is warm: the first
# probe build of a session compiles the stand-in library once.
"$tono" check "$proj/mathkit.tono" >/dev/null 2>&1 || true

TONO_BIN="$tono" "$driver" "$lsp" "$proj/mathkit.tono" --latency "$saves"

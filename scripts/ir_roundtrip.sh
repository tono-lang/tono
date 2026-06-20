#!/usr/bin/env bash
# Cross-language IR contract gate. Builds and tests both sides, then pipes each
# canonical example from the OCaml encoder through the Rust decoder/re-encoder
# and diffs the result. Any divergence exits non-zero and breaks the build.
set -euo pipefail

cd "$(dirname "$0")/.."

if command -v opam >/dev/null 2>&1; then
  eval "$(opam env)"
fi

echo "==> building both toolchains"
dune build
cargo build -p tono-backend

echo "==> OCaml tests (unit, property, golden fixtures)"
dune test

echo "==> Rust tests (fixture round-trip, negatives, version gate, sentinels)"
cargo test -p tono-backend

echo "==> live cross-language round-trip (OCaml encode -> Rust decode/re-encode)"
dump="_build/default/frontend/tools/dump_fixtures.exe"
mirror="target/debug/ir_roundtrip"
examples="list_charges nullable_charge open_enum_union primitives"
for name in $examples; do
  # The mirror decodes the frontend's JSON, re-encodes it, and exits non-zero
  # (printing the difference) if the two disagree as data.
  if "$dump" emit "$name" | "$mirror"; then
    echo "  ok: $name"
  else
    echo "DIVERGENCE in $name (see above)" >&2
    exit 1
  fi
done

echo "==> cross-language IR round-trip OK"

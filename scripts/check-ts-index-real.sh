#!/usr/bin/env bash
# Prove the TypeScript extractor on a real published package: install it
# (and the compiler API) into a scratch consumer tree, run `tono index` on
# a minimal spec that binds it, and check that the exports named on the
# command line are in the index with the expected kinds. Needs the network
# and node; run by hand, not in CI (which stays hermetic).
#
# usage: scripts/check-ts-index-real.sh <package> <version> <name>=<kind>...
#   e.g. scripts/check-ts-index-real.sh some-client 5.4.1 Client=class connect=function
set -euo pipefail

if [ $# -lt 3 ]; then
    echo "usage: $0 <package> <version> <name>=<kind>..." >&2
    exit 2
fi
package="$1"
version="$2"
shift 2

repo="$(cd "$(dirname "$0")/.." && pwd)"
frontend="${TONO_FRONTEND:-$repo/_build/default/frontend/bin/tono_frontend.exe}"
if [ ! -x "$frontend" ]; then
    echo "frontend not built at $frontend (run dune build, or set TONO_FRONTEND)" >&2
    exit 1
fi
tono="${TONO_BIN:-$repo/target/debug/tono}"
if [ ! -x "$tono" ]; then
    (cd "$repo" && cargo build -p tono-cli --quiet)
fi

work="$(mktemp -d "${TMPDIR:-/tmp}/tono-ts-index.XXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/sdk/ts"
(
    cd "$work/sdk/ts"
    npm init -y --silent >/dev/null
    npm install --silent --ignore-scripts "$package@$version" typescript@5 >/dev/null
)
ext="$(echo "$package" | sed 's#^@##; s#[^A-Za-z0-9]#_#g')"
cat >"$work/tono.toml" <<EOF
[project]
name = "probe"

[target.typescript]
out = "sdk/ts"

[ext.$ext]
ts = "$version"
EOF
cat >"$work/probe.tono" <<EOF
ext $ext {
  ts { #($package) }

  op probe(): string {
    ts { call: #(probe)() }
  }
}
EOF

start=$(date +%s.%N)
TONO_FRONTEND="$frontend" "$tono" index "$work/probe.tono" --json >"$work/report.jsonl"
end=$(date +%s.%N)
cat "$work/report.jsonl"
index="$work/.tono/index/$ext.ts.json"
if [ ! -f "$index" ]; then
    echo "no index written" >&2
    exit 1
fi
count=$(jq '.symbols | length' "$index")
note=$(jq -r '.note // "none"' "$index")
printf 'symbols: %s, note: %s, time: %.2fs, size: %s bytes\n' "$count" "$note" "$(echo "$end - $start" | bc)" "$(wc -c <"$index" | tr -d ' ')"

status=0
for expect in "$@"; do
    name="${expect%%=*}"
    kind="${expect#*=}"
    actual=$(jq -r --arg n "$name" '.symbols[] | select(.name == $n) | .kind' "$index")
    members=$(jq -r --arg n "$name" '.symbols[] | select(.name == $n) | .members | length' "$index")
    if [ "$actual" = "$kind" ]; then
        echo "ok: $name is a $kind ($members members)"
    else
        echo "MISSING: $name expected $kind, got ${actual:-nothing}" >&2
        status=1
    fi
done
exit $status

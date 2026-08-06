#!/usr/bin/env bash
# Runs the cross-runtime parity vectors (runtimes/parity/vectors.json) against
# a real generated SDK: compiles runtimes/parity/spec.tono, generates the SDK
# into a throwaway directory, drops the hand-written harness in next to it,
# and runs that target's native test suite.
#
# TypeScript and Go are repointed against the generated SDK; Rust still
# exercises its runtime package directly and slots in as another case later,
# without a rewrite.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
lang="${1:-typescript}"

case "$lang" in
typescript | go) ;;
rust)
    echo "run-parity.sh: rust is not repointed against a generated SDK yet; its share of the parity gate runs through 'cargo test' in runtimes/http-rust (set TONO_PARITY_VECTORS to require it)" >&2
    exit 1
    ;;
*)
    echo "run-parity.sh: unknown target '$lang' (expected typescript, go, or rust)" >&2
    exit 1
    ;;
esac

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

case "$lang" in
typescript)
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
    ;;
go)
    echo "generating the Go SDK..."
    "$tono" gen --target go --out "$work/sdk" --go-module parity.test/sdk <"$work/ir.json"

    echo "dropping the parity harness into the generated SDK..."
    # The harness joins the generated package (that is what lets it construct
    # through the unexported seam and pin the client's timing field); the
    # vectors sit next to the go.mod, one directory up from the package.
    cp "$root/runtimes/parity/go/parity_test.go" "$work/sdk/go/parity/"
    cp "$root/runtimes/parity/vectors.json" "$work/sdk/go/"

    echo "running the parity suite against the generated SDK..."
    (cd "$work/sdk/go" && go mod init parity.test/sdk >/dev/null 2>&1 && go mod tidy >/dev/null && go test ./...)
    ;;
esac

echo "parity suite passed against the generated $lang SDK"

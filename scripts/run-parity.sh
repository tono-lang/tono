#!/usr/bin/env bash
# Runs the cross-runtime parity vectors (runtimes/parity/vectors.json) against
# a real generated SDK: compiles runtimes/parity/spec.tono, generates the SDK
# into a throwaway directory, drops the hand-written harness in next to it,
# and runs that target's native test suite.
#
# Every target (TypeScript, Go, Rust) is repointed against the generated SDK,
# so the suite proves what a consumer actually imports, not that a
# hand-written runtime interprets a synthetic descriptor.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
lang="${1:-typescript}"

case "$lang" in
typescript | go | rust) ;;
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
    # Reuses the codegen test toolchain's own Vitest install rather than
    # provisioning a second one for a throwaway package; --root points
    # discovery at the generated tree instead of at that toolchain's own tests.
    if [ ! -d "$root/backend/codegen-tests/typescript/node_modules" ]; then
        (cd "$root/backend/codegen-tests/typescript" && npm ci --ignore-scripts)
    fi
    (cd "$root/backend/codegen-tests/typescript" && node node_modules/.bin/vitest run --root "$work/sdk/typescript")
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
rust)
    echo "generating the Rust SDK..."
    "$tono" gen --target rust --out "$work/sdk" <"$work/ir.json"

    echo "assembling the SDK crate with the parity harness..."
    # The harness runs as a #[cfg(test)] module of the generated crate itself:
    # that is what lets it reach the pub(crate) construction seam and pin the
    # client's crate-visible sleep/random pair (a tests/ integration test
    # would be a separate crate and see neither).
    mkdir -p "$work/crate/src"
    cp -R "$work/sdk/rust/." "$work/crate/src/"
    cp "$root/runtimes/parity/rust/parity_harness.rs" "$work/crate/src/"
    cp "$root/runtimes/parity/vectors.json" "$work/crate/src/"
    printf '#[cfg(test)]\nmod parity_harness;\n' >>"$work/crate/src/lib.rs"
    cat >"$work/crate/Cargo.toml" <<EOF
[package]
name = "parity_sdk"
version = "0.0.0"
edition = "2021"
[features]
default = ["reqwest"]
reqwest = ["dep:reqwest"]
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"], optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "time"] }
[workspace]
EOF

    echo "running the parity suite against the generated SDK..."
    (cd "$work/crate" && cargo test --quiet)
    ;;
esac

echo "parity suite passed against the generated $lang SDK"

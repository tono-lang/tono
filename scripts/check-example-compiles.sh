#!/usr/bin/env bash
# Verify the committed example SDKs actually compile in each language. The drift
# guard only proves the output is unchanged; this proves it is correct. Each SDK
# is built in a throwaway project so nothing leaks into the repo.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
sdk="examples/payments/sdk"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "rust..."
mkdir -p "$work/rust/src"
# The Rust SDK is split into a types module and a serde module of the same crate;
# copy both and declare them from the crate root.
cp "$sdk/rust/payments.rs" "$work/rust/src/payments.rs"
cp "$sdk/rust/payments_serde.rs" "$work/rust/src/payments_serde.rs"
cat >"$work/rust/src/lib.rs" <<'EOF'
pub mod payments;
pub mod payments_serde;
EOF
cat >"$work/rust/Cargo.toml" <<'EOF'
[package]
name = "example_rust"
version = "0.0.0"
edition = "2021"
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
[workspace]
EOF
(cd "$work/rust" && cargo build --quiet)

echo "go..."
mkdir -p "$work/go"
# The Go SDK is split into a types file and a serde file; both belong to the same
# package, so copy every generated .go file.
cp "$sdk"/go/*.go "$work/go/"
(cd "$work/go" && go mod init example_go >/dev/null 2>&1 && go build ./...)

echo "typescript..."
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
# The TypeScript SDK is split into a types module and a serde module; the serde
# file imports the types, so compile both together.
"$tsc" --noEmit --strict --target ES2020 --lib ES2020,DOM \
  "$sdk/typescript/payments.ts" "$sdk/typescript/payments_serde.ts"

echo "all three generated SDKs compile"

#!/usr/bin/env bash
# Verify the committed example SDKs actually compile in each language. The drift
# guard only proves the output is unchanged; this proves it is correct. Each SDK
# is built in a throwaway project so nothing leaks into the repo.
#
# The example is a two-module project, so the SDKs are laid out as sub-packages:
# Rust nested modules under a crate root, Go packages under a module path, and
# TypeScript files under sub-paths. The crate root (lib.rs) and the Go module path
# are the consumer's to provide; the codegen supplies the module tree beneath them.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
sdk="examples/payments/sdk"
go_module="example.com/sdk"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "rust..."
mkdir -p "$work/rust/src"
cp -R "$sdk"/rust/. "$work/rust/src/"
# The crate root declares each top-level generated module (a directory or a bare
# file); the generated mod.rs files declare everything beneath.
: >"$work/rust/src/lib.rs"
for entry in "$work"/rust/src/*; do
  base="$(basename "$entry")"
  [ "$base" = "lib.rs" ] && continue
  if [ -d "$entry" ]; then
    echo "pub mod $base;" >>"$work/rust/src/lib.rs"
  else
    echo "pub mod ${base%.rs};" >>"$work/rust/src/lib.rs"
  fi
done
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
# Deny warnings so a deprecated field (or any other lint) in the generated SDK
# fails here rather than in a downstream consumer's stricter build.
(cd "$work/rust" && RUSTFLAGS="-D warnings" cargo build --quiet)

echo "go..."
mkdir -p "$work/go"
cp -R "$sdk"/go/. "$work/go/"
# A driver that round-trips a charge through JSON, proving the cross-module union
# field (Method, whose type lives in the common package) actually decodes and not
# just that the package compiles.
mkdir -p "$work/go/verify"
cat >"$work/go/verify/main.go" <<EOF
package main

import (
	"encoding/json"
	"fmt"
	"os"

	"$go_module/payments/charges"
	"$go_module/payments/common"
)

func main() {
	orig := charges.Charge{Method: common.PaymentMethodCard{Value: common.Card{Last4: "4242"}}}
	b, err := json.Marshal(orig)
	if err != nil {
		fmt.Println("marshal:", err)
		os.Exit(1)
	}
	var back charges.Charge
	if err := json.Unmarshal(b, &back); err != nil {
		fmt.Println("unmarshal:", err)
		os.Exit(1)
	}
	card, ok := back.Method.(common.PaymentMethodCard)
	if !ok {
		fmt.Printf("method did not decode to a card variant: %T\n", back.Method)
		os.Exit(1)
	}
	if card.Value.Last4 != "4242" {
		fmt.Println("union payload did not survive the round trip:", card.Value.Last4)
		os.Exit(1)
	}
}
EOF
(cd "$work/go" && go mod init "$go_module" >/dev/null 2>&1 && go build ./... && go run ./verify)

echo "typescript..."
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
# The TypeScript SDK is a nested tree (a file per module) split into types and
# serde modules; the serde files import the types plus the hand-written HTTP
# runtime. A tsconfig maps the runtime package to its source and compiles every
# generated module together so cross-module and serde imports resolve, closing
# the Protocol/Target seam end to end.
mkdir -p "$work/ts"
cp -R "$sdk"/typescript/. "$work/ts/"
cat >"$work/ts/tsconfig.json" <<EOF
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true,
    "baseUrl": ".",
    "paths": { "@tono/http-runtime-ts": ["$root/runtimes/http-ts/src/index.ts"] }
  },
  "include": ["**/*.ts"]
}
EOF
(cd "$work/ts" && "$tsc" -p tsconfig.json)

echo "all three generated SDKs compile"

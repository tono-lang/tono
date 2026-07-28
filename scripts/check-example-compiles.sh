#!/usr/bin/env bash
# Verify the committed example SDKs actually compile in each language. The drift
# guard only proves the output is unchanged; this proves it is correct. Each SDK
# is built in a throwaway project so nothing leaks into the repo.
#
# The example is a two-module project, so the SDKs are laid out as emission
# groups: Rust modules of a crate (whose root the codegen now emits), Go packages
# under a module path, and TypeScript files under sub-paths with a package
# manifest. Only the Go module path is the consumer's to provide.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
sdk="examples/payments/sdk"
go_module="example.com/sdk"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "rust..."
mkdir -p "$work/rust/src"
# The crate root is generated: it declares each module and marks the shared
# internal group crate-visible, so nothing outside the crate can reach it.
cp -R "$sdk"/rust/. "$work/rust/src/"
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

# Rust fences with visibility rather than with a location: a declaration no
# public type reaches is `pub(crate)`, so a consumer crate cannot name it however
# it spells the path.
echo "rust visibility fence..."
mkdir -p "$work/rust-fence/src"
cat >"$work/rust-fence/src/main.rs" <<'EOF'
fn main() {
    let _: Option<example_rust::payments::charges::HTTPCode> = None;
}
EOF
cat >"$work/rust-fence/Cargo.toml" <<EOF
[package]
name = "rust_fence"
version = "0.0.0"
edition = "2021"
[dependencies]
example_rust = { path = "$work/rust" }
[workspace]
EOF
(
    cd "$work/rust-fence"
    if cargo build --quiet >"$work/rust-fence/err.txt" 2>&1; then
        echo "a crate-visible declaration must not be nameable from another crate" >&2
        exit 1
    fi
    if ! grep -qE "private|E0603|E0433" "$work/rust-fence/err.txt"; then
        echo "expected a privacy error, got:" >&2
        cat "$work/rust-fence/err.txt" >&2
        exit 1
    fi
)

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
# The generated entry client drives the hand-written Go HTTP runtime; the
# throwaway module resolves it from this repo, the way a consumer pins it.
# tidy keeps stderr so a resolution failure names its cause.
(cd "$work/go" && go mod init "$go_module" >/dev/null 2>&1 \
    && go mod edit -require=github.com/tono-lang/tono/runtimes/http-go@v0.0.0 \
    && go mod edit -replace=github.com/tono-lang/tono/runtimes/http-go="$root/runtimes/http-go" \
    && go mod tidy >/dev/null \
    && go build ./... && go run ./verify)

# Every group Go moves under internal/ (the SDK's shared helpers, and each
# module's own hidden declarations) is fenced off by the toolchain: a module
# outside the SDK cannot import it, however it spells the path.
echo "go internal fence..."
mkdir -p "$work/outside"
cat >"$work/outside/main.go" <<EOF
package main

import (
	_ "$go_module/internal/payments/charges"
	_ "$go_module/internal/tono"
)

func main() {}
EOF
(
    cd "$work/outside"
    go mod init example.com/outside >/dev/null 2>&1
    go mod edit -require="$go_module@v0.0.0"
    go mod edit -replace="$go_module=$work/go"
    # Resolution itself is where Go refuses the import, so both steps count as
    # the fence holding; only a clean build would mean it does not.
    if go mod tidy >"$work/outside/err.txt" 2>&1 && go build ./... >>"$work/outside/err.txt" 2>&1; then
        echo "the SDK's internal package must not be importable from outside it" >&2
        exit 1
    fi
    if ! grep -q internal "$work/outside/err.txt"; then
        echo "expected an internal-package error, got:" >&2
        cat "$work/outside/err.txt" >&2
        exit 1
    fi
)

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
    "paths": { "@tono/http-runtime-ts": ["$root/runtimes/http-ts/src/index.ts"] }
  },
  "include": ["**/*.ts"]
}
EOF
(cd "$work/ts" && "$tsc" -p tsconfig.json)

# TypeScript has no per-symbol visibility, so its fence is two things: the
# package's exports map, which Node refuses to resolve past, and what the
# module's barrel names, which is the only way in once the map is closed.
echo "typescript exports fence..."
fence="$work/ts-fence"
mkdir -p "$fence/node_modules/sdk"
cp -R "$sdk"/typescript/. "$fence/node_modules/sdk/"
cat >"$fence/probe.mjs" <<'EOF'
const refused = [
  "sdk/payments/charges/types",
  "sdk/payments/charges/codec",
  "sdk/codec",
  "sdk/config",
];
let bad = 0;
try {
  await import.meta.resolve("sdk/payments/charges");
} catch (e) {
  console.error(`the module barrel must resolve: ${e.code}`);
  bad++;
}
for (const spec of refused) {
  try {
    await import.meta.resolve(spec);
    console.error(`${spec} must not resolve: it is not in the exports map`);
    bad++;
  } catch (e) {
    if (e.code !== "ERR_PACKAGE_PATH_NOT_EXPORTED") {
      console.error(`${spec} failed for the wrong reason: ${e.code}`);
      bad++;
    }
  }
}
process.exit(bad === 0 ? 0 : 1);
EOF
(cd "$fence" && node probe.mjs)

# The other half of the fence: a declaration the barrel does not name has no way
# in, even though it shares a file with the module's public types.
echo "typescript barrel fence..."
mkdir -p "$work/ts-barrel"
cp -R "$fence/node_modules" "$work/ts-barrel/"
cat >"$work/ts-barrel/probe.ts" <<'EOF'
import { HTTPCode } from "sdk/payments/charges";
export const probe: HTTPCode = 200;
EOF
cat >"$work/ts-barrel/tsconfig.json" <<EOF
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "lib": ["ES2020", "DOM"],
    "module": "esnext",
    "moduleResolution": "bundler",
    "skipLibCheck": true,
    "allowImportingTsExtensions": true,
    "paths": { "@tono/http-runtime-ts": ["$root/runtimes/http-ts/src/index.ts"] }
  },
  "include": ["probe.ts"]
}
EOF
(
    cd "$work/ts-barrel"
    # The same probe importing a name the barrel does name compiles clean, so a
    # failure here is the fence and not the workspace.
    sed 's/HTTPCode/Charge/g;s/= 200;/| undefined = undefined;/' probe.ts >control.ts
    mv probe.ts fenced.ts && mv control.ts probe.ts
    if ! "$tsc" -p tsconfig.json >control.txt 2>&1; then
        echo "the barrel probe must compile for a name the barrel exports:" >&2
        cat control.txt >&2
        exit 1
    fi
    mv fenced.ts probe.ts.fenced && mv probe.ts control.ts && mv probe.ts.fenced probe.ts
    if "$tsc" -p tsconfig.json >err.txt 2>&1; then
        echo "a declaration the barrel does not name must not be importable" >&2
        exit 1
    fi
    if ! grep -q "HTTPCode" err.txt; then
        echo "expected an error naming HTTPCode, got:" >&2
        cat err.txt >&2
        exit 1
    fi
)

echo "auth-bearer..."
# The recipe is source only, so its Settings bridge only exists after a
# regeneration; this is its compile gate: frontend -> gen -> hook -> tsc.
#
# Go is built too even though the hook is bound for TypeScript only: this entry
# is the one whose fields resolve from the environment with nothing consuming
# them declaratively, which is exactly the shape that used to emit a Go
# constructor that would not compile.
frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [ ! -x "$frontend" ]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe)
fi
mkdir -p "$work/auth"
"$frontend" compile "$root/examples/auth-bearer/auth.tono" --module auth >"$work/auth/ir.json"
"$root/target/debug/tono" gen --target go,typescript --out "$work/auth/out" \
    --go-module example.com/auth "$work/auth/ir.json"
(cd "$work/auth/out/go" && go mod init example.com/auth >/dev/null 2>&1 \
    && go mod edit -require=github.com/tono-lang/tono/runtimes/http-go@v0.0.0 \
    && go mod edit -replace=github.com/tono-lang/tono/runtimes/http-go="$root/runtimes/http-go" \
    && go mod tidy >/dev/null \
    && go build ./...)
cp -R "$root/examples/auth-bearer/ext" "$work/auth/out/typescript/"
cat >"$work/auth/out/typescript/tsconfig.json" <<EOF
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true,
    "paths": { "@tono/http-runtime-ts": ["$root/runtimes/http-ts/src/index.ts"] }
  },
  "include": ["**/*.ts"]
}
EOF
(cd "$work/auth/out/typescript" && "$tsc" -p tsconfig.json)

echo "all generated SDKs compile"

#!/usr/bin/env bash
# Verify the committed example SDKs actually compile in each language. The drift
# guard only proves the output is unchanged; this proves it is correct.
#
# Every build here runs on the package `tono gen` wrote, exactly as it landed:
# the build manifest is the generated one, and nothing is written into the
# generated tree. What a consumer would add lives outside it (a driver crate or
# module, a tsconfig pointing in, a stand-in library pinned through a cargo
# `[patch]` in a parent config or a `go.work`). Committed SDKs are copied to a
# throwaway directory first so build artifacts never land in the repo.
set -euo pipefail

cd "$(dirname "$0")/.."
root="$PWD"
sdk="examples/payments/sdk"
go_module="example.com/sdk"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Every Rust build (the generated crates and their driver crates) shares one
# target directory, compiling the common dependencies once.
export CARGO_TARGET_DIR="$work/cargo-target"

tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
vitest="$root/backend/codegen-tests/typescript/node_modules/.bin/vitest"

# A tsconfig that typechecks a generated package from outside it: written to
# $1, including $2 (a directory relative to $1's own directory).
write_tsconfig() {
    cat >"$1" <<TSCONFIG
{
  "compilerOptions": {
    "strict": true,
    "noEmit": true,
    "target": "ES2020",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "lib": ["ES2020", "DOM"],
    "skipLibCheck": true
  },
  "include": ["$2/**/*.ts"],
  "exclude": ["**/*.test.ts", "**/node_modules/**"]
}
TSCONFIG
}

echo "rust..."
# The committed crate, built exactly as generated. Deny warnings so a
# deprecated field (or any other lint) in the generated SDK fails here rather
# than in a downstream consumer's stricter build.
cp -R "$sdk/rust" "$work/rust"
(cd "$work/rust" && RUSTFLAGS="-D warnings" cargo build --quiet \
    && RUSTFLAGS="-D warnings" cargo test --quiet)

# The reqwest feature is the default-on native transport; with it off, the
# generated SDK must still compile, with the canonical transport slot as the
# only way to send.
echo "rust --no-default-features..."
(cd "$work/rust" && RUSTFLAGS="-D warnings" cargo build --quiet --lib --no-default-features)

# A driver that constructs the generated entry client, points it at a local
# in-process server (via the entry's declared @env override, the only
# construction-time seam this entry exposes), and drives one real op call
# through the generated crate end to end: request assembly (header binding,
# endpoint ref), the transport call, and response decoding (including a
# cross-module union field, same as the Go verify driver). A consumer crate,
# depending on the generated package by path.
echo "rust runtime verify..."
mkdir -p "$work/rust-verify/src"
cp "$root/examples/payments/verify/verify.rs" "$work/rust-verify/src/main.rs"
cat >"$work/rust-verify/Cargo.toml" <<EOF
[package]
name = "verify"
version = "0.0.0"
edition = "2021"
[dependencies]
payments-sdk = { path = "$work/rust" }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "time"] }
[workspace]
EOF
(cd "$work/rust-verify" && RUSTFLAGS="-D warnings" cargo run --quiet)

# Rust fences with visibility rather than with a location: a declaration no
# public type reaches is `pub(crate)`, so a consumer crate cannot name it however
# it spells the path.
echo "rust visibility fence..."
mkdir -p "$work/rust-fence/src"
cat >"$work/rust-fence/src/main.rs" <<'EOF'
fn main() {
    let _: Option<payments_sdk::payments::charges::HTTPCode> = None;
}
EOF
cat >"$work/rust-fence/Cargo.toml" <<EOF
[package]
name = "rust_fence"
version = "0.0.0"
edition = "2021"
[dependencies]
payments-sdk = { path = "$work/rust" }
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
# The committed module, built exactly as generated: the go.mod is the one gen
# scaffolds, so no `go mod init` and no appended lines.
cp -R "$sdk/go" "$work/go"
(cd "$work/go" && go build ./... && go test ./...)

# A driver that round-trips a charge through JSON, proving the cross-module union
# field (Method, whose type lives in the common package) actually decodes and not
# just that the package compiles. A consumer module, pinning the SDK the way a
# consumer pins a private dependency.
mkdir -p "$work/go-verify"
cat >"$work/go-verify/main.go" <<EOF
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
cat >"$work/go-verify/go.mod" <<EOF
module example.com/verify

go 1.21

require $go_module v0.0.0
replace $go_module => $work/go
EOF
(cd "$work/go-verify" && go run .)

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
# The committed package, typechecked exactly as generated: the tsconfig sits
# outside the package and points in. No path mapping is needed: each entry
# carries its own transport.
mkdir -p "$work/ts"
cp -R "$sdk/typescript" "$work/ts/pkg"
write_tsconfig "$work/ts/tsconfig.json" "pkg"
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
  "sdk/number",
  "sdk/duration",
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
    "allowImportingTsExtensions": true
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
# regeneration; this is its compile gate: frontend -> gen -> the target
# toolchains on the output as it landed. No bespoke code is copied in: the
# header derives entirely from @format + @header.
#
# Go is built too: this entry is the one whose fields resolve from the
# environment with nothing consuming them declaratively, which is exactly the
# shape that used to emit a Go constructor that would not compile.
frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [ ! -x "$frontend" ]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe)
fi
mkdir -p "$work/auth"
"$frontend" compile "$root/examples/auth-bearer/auth.tono" --module auth >"$work/auth/ir.json"
"$root/target/debug/tono" gen --target go,typescript --out "$work/auth/out" \
    --package auth-sdk --go-module example.com/auth "$work/auth/ir.json"
(cd "$work/auth/out/go" && go build ./... && go test ./...)
write_tsconfig "$work/auth/tsconfig.json" "out/typescript"
(cd "$work/auth" && "$tsc" -p tsconfig.json)

echo "segmented-config..."
# The map-indexed match recipe: a field indexed by another field's value
# (`by_segment[.seg]`), its mandatory `null` arm, and `._` reading the
# looked-up value back. Compiled the same way as auth-bearer; both declared
# tests supply the map/key directly as construction values, so nothing
# depends on the environment.
mkdir -p "$work/segmented"
"$frontend" compile "$root/examples/segmented-config/service.tono" --module segmentedconfig >"$work/segmented/ir.json"
"$root/target/debug/tono" gen --target go,typescript --out "$work/segmented/out" \
    --package segmentedconfig-sdk --go-module example.com/segmentedconfig "$work/segmented/ir.json"
(cd "$work/segmented/out/go" && go build ./... && go test ./...)
write_tsconfig "$work/segmented/tsconfig.json" "out/typescript"
(cd "$work/segmented" && "$tsc" -p tsconfig.json)
(cd "$work/segmented/out/typescript" && "$vitest" run)

# The project manifest an ext example generates under: one enabled target per
# call, the ext version pins the generated manifests carry. Written next to
# the IR the way a real project's tono.toml sits next to its sources.
#   $1: directory  $2: project name  $3: go module path  $4: ext name
write_ext_tono_toml() {
    cat >"$1/tono.toml" <<TOML
[project]
name = "$2"

[target.go]
enabled = true
package = "$3"
out = "out/go"

[target.typescript]
enabled = true
package = "$2"
out = "out/typescript"

[target.rust]
enabled = true
package = "$2"
out = "out/rust"

[ext.$4]
go = "v0.0.0"
ts = "0.0.0"
rust = "0.0.0"
TOML
}

echo "config-lib..."
# The `ext <lib> { extern ... }` FFI recipe: a config field constructed by a
# declarative call into a third-party library, exercised for real against a
# stand-in package per target (under ext/), pinned the way a consumer pins a
# private dependency: a `go.work` beside the output, a cargo `[patch]` in the
# parent config, a node_modules one level up. Every generated declared test
# stubs the library call (`stub configlib.load`), so this also proves each
# target's construction override for an extern-call field, not just that the
# SDK compiles.
mkdir -p "$work/config-lib"
"$frontend" compile "$root/examples/config-lib/service.tono" --module configsvc >"$work/config-lib/ir.json"
write_ext_tono_toml "$work/config-lib" configsvc-sdk example.com/configsvc configlib
"$root/target/debug/tono" gen --config "$work/config-lib/tono.toml" "$work/config-lib/ir.json"

(cd "$work/config-lib" && go work init ./out/go >/dev/null && go work use "$root/examples/config-lib/ext/go")
(cd "$work/config-lib/out/go" && go build ./... && go test ./...)

mkdir -p "$work/config-lib/.cargo"
cat >"$work/config-lib/.cargo/config.toml" <<EOF
[patch.crates-io]
configlib = { path = "$root/examples/config-lib/ext/rust" }
EOF
(cd "$work/config-lib/out/rust" && cargo build --quiet && cargo test --quiet)

mkdir -p "$work/config-lib/node_modules/@example"
cp -R "$root/examples/config-lib/ext/ts/configlib" "$work/config-lib/node_modules/@example/"
write_tsconfig "$work/config-lib/tsconfig.json" "out/typescript"
(cd "$work/config-lib" && "$tsc" -p tsconfig.json)
(cd "$work/config-lib/out/typescript" && "$vitest" run)

echo "settings-source..."
# The foreign-generic-instantiation recipe: an opaque handle names which
# instantiation of a foreign generic type it declares
# (type env_source("Source", app_settings) { ... }), so the emitted Go names
# the concrete type argument (*settingskit.Source[AppSettings],
# settingskit.NewEnvSource[AppSettings](...)) instead of leaving it for the
# compiler to infer, against a real (if stand-in) generic package per target.
# The declared test stubs the handle method the operation's own `impl` body
# calls (`stub settingskit.env_source.get`), so this also proves the hermetic
# handle-method stub in Go (the fake handle) and TypeScript (the op's seam);
# the Rust target generates no test for that combination (no seam to swap),
# so its SDK only has to compile.
mkdir -p "$work/settings-source"
"$frontend" compile "$root/examples/settings-source/service.tono" --module settingssource >"$work/settings-source/ir.json"
write_ext_tono_toml "$work/settings-source" settingssource-sdk example.com/settingssource settingskit
"$root/target/debug/tono" gen --config "$work/settings-source/tono.toml" "$work/settings-source/ir.json"

(cd "$work/settings-source" && go work init ./out/go >/dev/null && go work use "$root/examples/settings-source/ext/go")
(cd "$work/settings-source/out/go" && go build ./... && go test ./...)

mkdir -p "$work/settings-source/.cargo"
cat >"$work/settings-source/.cargo/config.toml" <<EOF
[patch.crates-io]
settingskit = { path = "$root/examples/settings-source/ext/rust" }
EOF
(cd "$work/settings-source/out/rust" && cargo build --quiet && cargo test --quiet)

mkdir -p "$work/settings-source/node_modules/@example"
cp -R "$root/examples/settings-source/ext/ts/settingskit" "$work/settings-source/node_modules/@example/"
write_tsconfig "$work/settings-source/tsconfig.json" "out/typescript"
(cd "$work/settings-source" && "$tsc" -p tsconfig.json)
(cd "$work/settings-source/out/typescript" && "$vitest" run)

echo "provider-config..."
# The handle-method field-source recipe: a foreign provider constructed
# once, one of its methods read into a field (`endpoints: endpoints =
# .provider.get()`) that several `@http` operations consume through their
# endpoint, against a real (if stand-in) Go package. The declared test stubs
# both the constructor and the method reached at construction, so this also
# proves the hermetic fake honours the method's `ctx` slot.
mkdir -p "$work/provider-config"
"$frontend" compile "$root/examples/provider-config/service.tono" --module providerconfig >"$work/provider-config/ir.json"
cat >"$work/provider-config/tono.toml" <<TOML
[project]
name = "providerconfig-sdk"

[target.go]
enabled = true
package = "example.com/providerconfig"
out = "out/go"

[ext.envkit]
go = "v0.0.0"
TOML
"$root/target/debug/tono" gen --config "$work/provider-config/tono.toml" "$work/provider-config/ir.json"
(cd "$work/provider-config" && go work init ./out/go >/dev/null && go work use "$root/examples/provider-config/ext/go")
(cd "$work/provider-config/out/go" && go build ./... && go test ./...)

echo "all generated SDKs compile"

echo "ffi bench..."
# The FFI bench (examples/mathkit): one library declared with the shapes real
# libraries have, taken through frontend, generation, the target compiler
# against the stand-in packages, the declared tests, and a driver that runs
# the SDK for real. Its checks are expected red until the emitters catch up;
# examples/mathkit/gate.tsv records what each must reach, and any drift from
# that record (regression or progress) fails here.
scripts/check-ffi-bench.sh

#!/usr/bin/env bash
# The FFI bench: examples/mathkit declares one numeric library with the shapes
# real libraries have (interface handle and variadic options in Go, one concrete
# struct per constructor in Rust, classes and a synchronous method in
# TypeScript), and this gate takes it the whole way: frontend, generation, the
# target compiler against the stand-in library, the generated declared tests,
# and a driver that runs the SDK against the library for real.
#
# The bench was written before the emitters could pass it, so most checks are
# expected red today. examples/mathkit/gate.tsv records the outcome each check
# must reach; a check that ends anywhere else fails this gate, whether it
# regressed or started passing (then the record and README.md are updated
# together). Nothing is skipped: every red row is printed with the stage it
# stopped at and the reason.
set -uo pipefail

cd "$(dirname "$0")/.." || exit 1
root="$PWD"
bench="$root/examples/mathkit"
fixtures="$root/backend/codegen-tests"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [ ! -x "$frontend" ]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe)
fi
tono="$root/target/debug/tono"
if [ ! -x "$tono" ]; then
    (cd "$root" && cargo build -p tono-cli --quiet)
fi
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
vitest="$root/backend/codegen-tests/typescript/node_modules/.bin/vitest"
for tool in "$tsc" "$vitest"; do
    if [ ! -x "$tool" ]; then
        echo "$(basename "$tool") is not installed; run 'npm ci' in backend/codegen-tests/typescript" >&2
        exit 1
    fi
done
for tool in go cargo; do
    if ! command -v "$tool" >/dev/null; then
        echo "$tool is not installed" >&2
        exit 1
    fi
done

# The TypeScript formatter is the pinned prettier from the codegen test
# toolchain, the same one the committed examples are formatted with.
export PATH="$root/backend/codegen-tests/typescript/node_modules/.bin:$PATH"

# Every Rust check wraps the generated modules in its own throwaway crate with
# the same dependencies; one target directory per run compiles those once.
export CARGO_TARGET_DIR="$work/cargo-target"

# The generation-time refusal capability 10 asserts: a handle forwarded to
# another call is owned by that call, so a second reader is refused.
ownership_marker="is owned by that call"

# One check. Prints the outcome it reached to stdout; every tool log goes to
# $work/<id>/log so a mismatch can quote it.
run_check() {
    local id="$1" target="$2" source="$3"
    local dir="$work/$id"
    mkdir -p "$dir"
    local log="$dir/log"
    : >"$log"

    if ! "$frontend" compile "$bench/$source" --module mathkit >"$dir/ir.json" 2>>"$log"; then
        echo frontend-red
        return
    fi
    if ! "$tono" gen --target "$target" --out "$dir/out" --go-module example.com/mathkit "$dir/ir.json" >>"$log" 2>&1; then
        if grep -q "$ownership_marker" "$log"; then
            echo refused
        else
            echo gen-red
        fi
        return
    fi

    local driver_base="$bench/probes/$id.verify"
    if [ "$id" = bench ]; then
        driver_base="$bench/verify/verify"
    fi
    case "$target" in
    go) run_go "$dir" "$driver_base.go" ;;
    rust) run_rust "$dir" "$driver_base.rs" ;;
    typescript) run_typescript "$dir" "$driver_base.test.ts" ;;
    esac
}

# Build the generated Go against the stand-in package (a `replace` at the
# import path the .tono declares, the way a consumer pins a private
# dependency), run the generated declared tests, then the driver.
run_go() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local sdk="$dir/out/go"
    (
        cd "$sdk" && go mod init example.com/mathkit >/dev/null 2>&1 &&
            cat >>go.mod <<GOMOD
require tono-ext-fixture/mathkit v0.0.0
replace tono-ext-fixture/mathkit => $fixtures/go-ext/fixtures/mathkit
GOMOD
        go mod tidy >/dev/null 2>&1 && go build ./...
    ) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    (cd "$sdk" && go test ./...) >>"$log" 2>&1 || {
        echo test-red
        return
    }
    if [ ! -f "$driver" ]; then
        echo no-driver
        return
    fi
    mkdir -p "$sdk/verify"
    cp "$driver" "$sdk/verify/main.go"
    (cd "$sdk" && go mod tidy >/dev/null 2>&1 && go run ./verify) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

# Wrap the generated Rust modules in a crate that depends on the stand-in
# crate by path, build it, run its declared tests, then the driver binary.
run_rust() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local crate="$dir/crate"
    mkdir -p "$crate/src"
    cp -R "$dir/out/rust/." "$crate/src/"
    cat >"$crate/Cargo.toml" <<CARGO
[package]
name = "example_mathkit"
version = "0.0.0"
edition = "2021"
[lib]
name = "example_mathkit"
path = "src/lib.rs"
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tono_ext = { package = "sdk-ext-runtime-rs", path = "$root/runtimes/ext-rust" }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"], optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "time"] }
mathkit = { path = "$fixtures/rust-ext/fixtures/mathkit" }
[features]
default = ["reqwest"]
reqwest = ["dep:reqwest"]
[workspace]
CARGO
    (cd "$crate" && cargo build --quiet) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    (cd "$crate" && cargo test --quiet) >>"$log" 2>&1 || {
        echo test-red
        return
    }
    if [ ! -f "$driver" ]; then
        echo no-driver
        return
    fi
    mkdir -p "$crate/verify"
    cp "$driver" "$crate/verify/main.rs"
    cat >>"$crate/Cargo.toml" <<CARGO
[[bin]]
name = "verify"
path = "verify/main.rs"
CARGO
    (cd "$crate" && cargo run --quiet --bin verify) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

# Install the stand-in package into a throwaway node_modules (the way a
# consumer's install would place it), typecheck the generated modules, run
# the generated declared tests, then the driver test.
run_typescript() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local sdk="$dir/out/typescript"
    mkdir -p "$sdk/node_modules/@tono-ext-fixture"
    cp -R "$fixtures/ts-ext/fixtures/mathkit" "$sdk/node_modules/@tono-ext-fixture/"
    cat >"$sdk/tsconfig.json" <<TSCONFIG
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
  "include": ["**/*.ts"],
  "exclude": ["**/*.test.ts"]
}
TSCONFIG
    (cd "$sdk" && "$tsc" -p tsconfig.json) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    if [ -n "$(find "$sdk" -name '*.test.ts' -not -path '*/node_modules/*' | head -1)" ]; then
        (cd "$sdk" && "$vitest" run) >>"$log" 2>&1 || {
            echo test-red
            return
        }
    fi
    if [ ! -f "$driver" ]; then
        echo no-driver
        return
    fi
    cp "$driver" "$sdk/verify.test.ts"
    (cd "$sdk" && "$vitest" run verify.test.ts) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

mismatches=0
printf '%-28s %-11s %-13s %-13s\n' check target expected actual
while IFS=$'\t' read -r id target source expected; do
    case "$id" in "" | \#*) continue ;; esac
    actual="$(run_check "$id" "$target" "$source")"
    if [ "$actual" = "$expected" ]; then
        verdict=""
    else
        verdict="  <-- MISMATCH"
        mismatches=$((mismatches + 1))
    fi
    printf '%-28s %-11s %-13s %-13s%s\n' "$id" "$target" "$expected" "$actual" "$verdict"
    if [ "$actual" != pass ] || [ -n "$verdict" ]; then
        # The reason a check stopped, so a red row is never a bare word: the
        # first lines of the tool that refused it.
        sed -n '1,12p' "$work/$id/log" | sed 's/^/    | /'
    fi
done <"$bench/gate.tsv"

if [ "$mismatches" -ne 0 ]; then
    echo "ffi bench: $mismatches check(s) ended elsewhere than examples/mathkit/gate.tsv records" >&2
    echo "a red row that now passes is progress: update gate.tsv and README.md together" >&2
    exit 1
fi
echo "ffi bench: every check matches its recorded outcome"

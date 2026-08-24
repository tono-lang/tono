#!/usr/bin/env bash
# The FFI bench: examples/mathkit declares one numeric library with the shapes
# real libraries have (interface handle and variadic options in Go, one concrete
# struct per constructor in Rust, classes and a synchronous method in
# TypeScript), and this gate takes it the whole way: frontend, the binding
# check against the stand-in library (tono check), generation, the target
# compiler against the stand-in library, the generated declared tests, and a
# driver that runs the SDK against the library for real.
#
# Every check compiles the package `tono gen` wrote, exactly as it landed: the
# build manifest is the generated one, and the stand-in library is pinned the
# way a consumer pins a private dependency, from outside the package (a cargo
# `[patch]` in a parent config, a `go.work`, a `node_modules` one level up).
# The harness writes nothing into the generated tree.
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
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

frontend="$root/_build/default/frontend/bin/tono_frontend.exe"
if [[ ! -x "$frontend" ]]; then
    (cd "$root" && opam exec -- dune build frontend/bin/tono_frontend.exe)
fi
tono="$root/target/debug/tono"
if [[ ! -x "$tono" ]]; then
    (cd "$root" && cargo build -p tono-cli --quiet)
fi
tsc="$root/backend/codegen-tests/typescript/node_modules/.bin/tsc"
vitest="$root/backend/codegen-tests/typescript/node_modules/.bin/vitest"
for tool in "$tsc" "$vitest"; do
    if [[ ! -x "$tool" ]]; then
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

# One target directory per run compiles the shared dependencies once across
# the generated crates and their verify drivers.
export CARGO_TARGET_DIR="$work/cargo-target"

# `tono check` resolves each language's library the way the generated SDK
# does: from a consumer tree. One Go module requiring the stand-in through a
# `replace`, one node_modules holding the stand-in package, both built once.
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

# The generation-time refusals the bench asserts, each by the phrase the
# generator names the rule with: capability 10 (a handle forwarded to
# another call is owned by that call, so a second reader is refused),
# capability 11 on Go (a static method receiver has nothing to render as,
# Go has no static method), capability 12 on Go and Rust (a class
# reference has nothing to render as, neither has a type as a value), and
# capability 15 on Rust (a spelling asking for a conversion the target
# cannot write is refused naming both types).
refusal_markers="is owned by that call|has no static method to call|has no class reference to pass|no conversion from"

# The project manifest each check generates under: one enabled target, the
# ext version pins the generated manifest carries, written next to the IR the
# way a real project's tono.toml sits next to its sources.
write_tono_toml() {
    local dir="$1" target="$2"
    local package="mathkit-sdk"
    if [[ "$target" = go ]]; then
        package="example.com/mathkit"
    fi
    cat >"$dir/tono.toml" <<TOML
[project]
name = "mathkit-sdk"

[target.$target]
enabled = true
package = "$package"
out = "out/$target"

[ext.mathkit]
rust = "0.0.0"
go = "v0.0.0"
ts = "0.0.0"
TOML
}

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
    # The bindings against the stand-in library, reported on the .tono. A
    # finding here is what the target compiler would have said about a
    # generated line; the gate wants it said about the declaration instead.
    if ! "$tono" check "$bench/$source" --lib-root "go=$check_go" --lib-root "ts=$check_ts" >>"$log" 2>&1; then
        echo check-red
        return
    fi
    write_tono_toml "$dir" "$target"
    if ! "$tono" gen --config "$dir/tono.toml" "$dir/ir.json" >>"$log" 2>&1; then
        if grep -Eq "$refusal_markers" "$log"; then
            echo refused
        else
            echo gen-red
        fi
        return
    fi

    local driver_base="$bench/probes/$id.verify"
    if [[ "$id" = bench ]]; then
        driver_base="$bench/verify/verify"
    fi
    case "$target" in
    go) run_go "$dir" "$driver_base.go" ;;
    rust) run_rust "$dir" "$driver_base.rs" ;;
    typescript) run_typescript "$dir" "$driver_base.test.ts" ;;
    *) echo "unknown-target" ;;
    esac
}

# Build the generated Go package as it landed. A `go.work` beside the output
# pins the stand-in at the import path the .tono declares, the way a consumer
# workspace pins a private dependency; the generated go.mod is not edited.
# The driver is its own module in the same workspace, importing the SDK by
# its module path from outside it.
run_go() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local sdk="$dir/out/go"
    (cd "$dir" && go work init ./out/go && go work use "$bench/ext/go") >>"$log" 2>&1
    (cd "$sdk" && go build ./...) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    (cd "$sdk" && go test ./...) >>"$log" 2>&1 || {
        echo test-red
        return
    }
    if [[ ! -f "$driver" ]]; then
        echo no-driver
        return
    fi
    mkdir -p "$dir/verify"
    cp "$driver" "$dir/verify/main.go"
    cat >"$dir/verify/go.mod" <<GOMOD
module example.com/verify

go 1.21

require example.com/mathkit v0.0.0
GOMOD
    (cd "$dir" && go work use ./verify && cd verify && go run .) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

# Build the generated crate as it landed. The stand-in crate is pinned from
# outside the package: a cargo `[patch]` in the parent directory's config,
# which cargo discovers walking up from the crate. The driver is its own
# consumer crate depending on the SDK by path.
run_rust() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local sdk="$dir/out/rust"
    mkdir -p "$dir/.cargo"
    cat >"$dir/.cargo/config.toml" <<CFG
[patch.crates-io]
mathkit = { path = "$bench/ext/rust" }
CFG
    (cd "$sdk" && cargo build --quiet) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    (cd "$sdk" && cargo test --quiet) >>"$log" 2>&1 || {
        echo test-red
        return
    }
    if [[ ! -f "$driver" ]]; then
        echo no-driver
        return
    fi
    mkdir -p "$dir/verify/src"
    cp "$driver" "$dir/verify/src/main.rs"
    cat >"$dir/verify/Cargo.toml" <<CARGO
[package]
name = "verify"
version = "0.0.0"
edition = "2021"
[dependencies]
mathkit-sdk = { path = "../out/rust" }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
[workspace]
CARGO
    (cd "$dir/verify" && cargo run --quiet) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

# Typecheck the generated package as it landed: the tsconfig sits outside the
# package and points in, the stand-in package sits in a node_modules one
# level up (where module resolution walks), and the SDK itself is linked
# there under its package name so the driver imports it as a consumer.
run_typescript() {
    local dir="$1" driver="$2"
    local log="$dir/log"
    local sdk="$dir/out/typescript"
    mkdir -p "$dir/node_modules/@tono-ext-fixture"
    # The package's own name is "@tono-ext-fixture/mathkit" (ext/ts/package.json),
    # not "ts" (the source directory's own name): node module resolution walks
    # node_modules/<package name>, so the copy must land under that name, not
    # the source directory's own.
    cp -R "$bench/ext/ts" "$dir/node_modules/@tono-ext-fixture/mathkit"
    ln -s ../out/typescript "$dir/node_modules/mathkit-sdk"
    cat >"$dir/tsconfig.json" <<TSCONFIG
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
  "include": ["out/typescript/**/*.ts"],
  "exclude": ["**/*.test.ts", "**/node_modules/**"]
}
TSCONFIG
    (cd "$dir" && "$tsc" -p tsconfig.json) >>"$log" 2>&1 || {
        echo build-red
        return
    }
    if [[ -n "$(find "$sdk" -name '*.test.ts' -not -path '*/node_modules/*' | head -1)" ]]; then
        (cd "$sdk" && "$vitest" run) >>"$log" 2>&1 || {
            echo test-red
            return
        }
    fi
    if [[ ! -f "$driver" ]]; then
        echo no-driver
        return
    fi
    cp "$driver" "$dir/verify.test.ts"
    (cd "$dir" && "$vitest" run verify.test.ts) >>"$log" 2>&1 || {
        echo run-red
        return
    }
    echo pass
}

mismatches=0
printf '%-28s %-11s %-13s %-13s\n' check target expected actual
while IFS=$'\t' read -r id target source expected; do
    case "$id" in
    "" | \#*) continue ;;
    *) ;;
    esac
    actual="$(run_check "$id" "$target" "$source")"
    if [[ "$actual" = "$expected" ]]; then
        verdict=""
    else
        verdict="  <-- MISMATCH"
        mismatches=$((mismatches + 1))
    fi
    printf '%-28s %-11s %-13s %-13s%s\n' "$id" "$target" "$expected" "$actual" "$verdict"
    if [[ "$actual" != pass ]] || [[ -n "$verdict" ]]; then
        # The reason a check stopped, so a red row is never a bare word: the
        # first lines of the tool that refused it.
        sed -n '1,12p' "$work/$id/log" | sed 's/^/    | /'
    fi
done <"$bench/gate.tsv"

if [[ "$mismatches" -ne 0 ]]; then
    echo "ffi bench: $mismatches check(s) ended elsewhere than examples/mathkit/gate.tsv records" >&2
    echo "a red row that now passes is progress: update gate.tsv and README.md together" >&2
    exit 1
fi
echo "ffi bench: every check matches its recorded outcome"

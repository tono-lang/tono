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
cat >"$work/rust/Cargo.toml" <<EOF
[package]
name = "example_rust"
version = "0.0.0"
edition = "2021"
[lib]
name = "example_rust"
path = "src/lib.rs"
[[bin]]
name = "verify"
path = "verify/main.rs"
[features]
default = ["reqwest"]
reqwest = ["dep:reqwest"]
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tono_ext = { package = "sdk-ext-runtime-rs", path = "$root/runtimes/ext-rust" }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"], optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "time"] }
[workspace]
EOF
# A driver that constructs the generated entry client, points it at a local
# in-process server (via the entry's declared @env override, the only
# construction-time seam this entry exposes), and drives one real op call
# through the hand-written Rust HTTP runtime end to end: request assembly
# (header binding, endpoint ref), the transport call, and response decoding
# (including a cross-module union field, same as the Go verify driver).
mkdir -p "$work/rust/verify"
cat >"$work/rust/verify/main.rs" <<'EOF'
use example_rust::payments::charges::{Charge, Client};
use example_rust::payments::common::{Card, PaymentMethod, Status};
use example_rust::support::Timestamp;

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();

    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = socket.read(&mut chunk).await.expect("read headers");
            assert!(n > 0, "connection closed before headers completed");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length: usize = headers
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().expect("content-length"))
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let n = socket.read(&mut chunk).await.expect("read body");
            assert!(n > 0, "connection closed before body completed");
            buf.extend_from_slice(&chunk[..n]);
        }
        let body = String::from_utf8_lossy(&buf[body_start..body_start + content_length]).to_string();

        assert!(
            headers.starts_with("POST /charges "),
            "unexpected request line: {headers}"
        );
        assert!(
            headers.to_ascii_lowercase().contains("x-api-key: test-key"),
            "declared header did not resolve from the client values: {headers}"
        );
        // i64 rides the wire as a string, not a bare JSON number.
        assert!(
            body.contains("\"amount\":\"1000\""),
            "request body missing the bound member: {body}"
        );

        // i64/u64 ride the wire as strings, not bare JSON numbers.
        let response_body = "{\"id\":\"c1\",\"amount\":\"1000\",\"fee\":\"0\",\"receipt\":\"aGk=\",\"currency\":\"usd\",\
             \"tags\":[],\"metadata\":{},\"created\":\"2024-01-01T00:00:00Z\",\"status\":\"active\",\
             \"method\":{\"kind\":\"card\",\"last4\":\"4242\"}}";
        // create_charge declares @http(code: 201): the server must answer
        // exactly that status for the client to decode the body as a success.
        let response = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body,
        );
        socket.write_all(response.as_bytes()).await.expect("write response");
        socket.flush().await.expect("flush");
    });

    // The entry's base URL resolves only from @env (no @with override is
    // declared for it), so the env var is the construction-time seam this
    // verify driver has to point at the local server.
    std::env::set_var("PAYMENTS_ENDPOINT", format!("http://127.0.0.1:{port}"));

    let client = Client::builder("test-key".to_string())
        .build()
        .expect("build client");

    // `fee` is @deprecated on the schema itself (folded into `amount`); a
    // consumer still has to set it (the field is not optional), same as any
    // other caller migrating off it.
    #[allow(deprecated)]
    let input = Charge {
        id: "ignored-by-the-server".to_string(),
        amount: 1000,
        fee: 0,
        receipt: b"hi".to_vec(),
        currency: "usd".to_string(),
        note: None,
        tags: vec![],
        metadata: Default::default(),
        created: Timestamp("2024-01-01T00:00:00Z".to_string()),
        status: Status::Active,
        method: PaymentMethod::Card(Card {
            last4: "4242".to_string(),
        }),
    };

    let charge = client.create_charge(input).await.expect("create_charge");
    assert_eq!(charge.id, "c1");
    assert_eq!(charge.amount, 1000);
    assert_eq!(charge.currency, "usd");
    assert_eq!(charge.receipt, b"hi");
    match charge.method {
        PaymentMethod::Card(card) => assert_eq!(card.last4, "4242"),
        other => panic!("union payload did not survive the round trip: {other:?}"),
    }

    server.await.expect("server task");
    println!("rust runtime verify: ok");
}
EOF
# Deny warnings so a deprecated field (or any other lint) in the generated SDK
# fails here rather than in a downstream consumer's stricter build.
(cd "$work/rust" && RUSTFLAGS="-D warnings" cargo build --quiet \
    && RUSTFLAGS="-D warnings" cargo run --quiet --bin verify \
    && RUSTFLAGS="-D warnings" cargo test --quiet)

# The reqwest feature is the default-on native transport; with it off, the
# generated SDK must still compile, with the canonical transport slot as the
# only way to send.
echo "rust --no-default-features..."
(cd "$work/rust" && RUSTFLAGS="-D warnings" cargo build --quiet --lib --no-default-features)

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
    && go mod tidy >/dev/null \
    && go build ./... && go run ./verify && go test ./...)

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
vitest="$root/backend/codegen-tests/typescript/node_modules/.bin/vitest"
# The TypeScript SDK is a nested tree (a file per module) split into types and
# serde modules; a tsconfig compiles every generated module together so
# cross-module and serde imports resolve, closing the Protocol/Target seam end
# to end. No path mapping is needed: each entry carries its own transport.
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
    "skipLibCheck": true
  },
  "include": ["**/*.ts"],
  "exclude": ["**/*.test.ts"]
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
# regeneration; this is its compile gate: frontend -> gen -> tsc. No bespoke
# code is copied in: the header derives entirely from @format + @header.
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
    --go-module example.com/auth "$work/auth/ir.json"
(cd "$work/auth/out/go" && go mod init example.com/auth >/dev/null 2>&1 \
    && go mod tidy >/dev/null \
    && go build ./...)
(cd "$work/auth/out/go" && go test ./...)
cat >"$work/auth/out/typescript/tsconfig.json" <<EOF
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
EOF
(cd "$work/auth/out/typescript" && "$tsc" -p tsconfig.json)

echo "config-lib..."
# The `ext <lib> { extern ... }` FFI recipe: a config field constructed by a
# declarative call into a third-party library, exercised for real against a
# stand-in package per target (under ext/), the way a real dependency would
# be pinned. Every generated declared test stubs the library call
# (`stub configlib.load`), so this also proves each target's construction
# override for an extern-call field, not just that the SDK compiles.
mkdir -p "$work/config-lib"
"$frontend" compile "$root/examples/config-lib/service.tono" --module configsvc >"$work/config-lib/ir.json"
"$root/target/debug/tono" gen --target go,typescript,rust --out "$work/config-lib/out" \
    --go-module example.com/configsvc "$work/config-lib/ir.json"

(cd "$work/config-lib/out/go" && go mod init example.com/configsvc >/dev/null 2>&1 \
    && cat >>go.mod <<EOF
require github.com/example/configlib v0.0.0
replace github.com/example/configlib => $root/examples/config-lib/ext/go
EOF
    go mod tidy >/dev/null \
    && go build ./...)
(cd "$work/config-lib/out/go" && go test ./...)

mkdir -p "$work/config-lib/rust-sdk/src"
cp -R "$work/config-lib/out/rust/." "$work/config-lib/rust-sdk/src/"
cat >"$work/config-lib/rust-sdk/Cargo.toml" <<EOF
[package]
name = "example_configsvc"
version = "0.0.0"
edition = "2021"
[lib]
name = "example_configsvc"
path = "src/lib.rs"
[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tono_ext = { package = "sdk-ext-runtime-rs", path = "$root/runtimes/ext-rust" }
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"], optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros", "net", "io-util", "time"] }
configlib = { path = "$root/examples/config-lib/ext/rust" }
[features]
default = ["reqwest"]
reqwest = ["dep:reqwest"]
[workspace]
EOF
(cd "$work/config-lib/rust-sdk" && cargo build --quiet && cargo test --quiet)

mkdir -p "$work/config-lib/out/typescript/node_modules/@example"
cp -R "$root/examples/config-lib/ext/ts/configlib" "$work/config-lib/out/typescript/node_modules/@example/"
cat >"$work/config-lib/out/typescript/tsconfig.json" <<EOF
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
EOF
(cd "$work/config-lib/out/typescript" && "$tsc" -p tsconfig.json)
(cd "$work/config-lib/out/typescript" && "$vitest" run)

echo "all generated SDKs compile"

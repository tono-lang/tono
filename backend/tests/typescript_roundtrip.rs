//! End-to-end check that the TypeScript the engine emits compiles and round-trips.
//!
//! Generates a module, formats it with prettier, writes it plus a hand-written
//! driver, compiles everything with tsc, and runs the driver with node. The
//! driver asserts the hard wire cases hold (i64 above 2^53 as a string, bytes as
//! base64, open-enum lenient decode, the identifier vs wire-key split). Skips
//! cleanly if the toolchain is absent; CI installs it (see
//! backend/codegen-tests/typescript).

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::render::render_file_with_companion;
use tono_backend::codegen::targets::typescript::emit::emit_module;
use tono_backend::codegen::targets::typescript::types::ts_casing;
use tono_backend::codegen::targets::typescript::TsRules;
use tono_backend::codegen::Formatter;

mod common;
use common::matrix_module as demo_module;

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/typescript")
}

fn tool(ws: &Path, name: &str) -> Option<PathBuf> {
    let bin = ws.join("node_modules/.bin").join(name);
    bin.exists().then_some(bin)
}

const DRIVER: &str = r#"
import { Account, APIError, PaymentDeclinedError, RateLimitedError, TonoError } from "./models";
import { encodeAccount, decodeAccount, decodeCreateChargeError } from "./models_serde";

const big = 9007199254740993n; // 2^53 + 1, not representable as a JS number

const account: Account = {
  accountID: big,
  secret: new Uint8Array([1, 2, 3, 254]),
  tip: 500n,
  status: "active",
  code: 200,
  method: { type: "card", last4: "4242" },
  counts: [[7, "a"], [3, "b"]],
};

const wire: any = encodeAccount(account);
if (typeof wire.account_id !== "string") throw new Error("i64 must encode to a string");
if (wire.account_id !== big.toString()) throw new Error("i64 wire value wrong");
if (wire.tip !== "500") throw new Error("an optional i64 must encode to a string");
if (wire.code !== 200) throw new Error("an int-backed enum must encode as a bare number");
if (wire.method.type !== "card") throw new Error("a union must carry its discriminator");
if (JSON.stringify(wire.counts) !== JSON.stringify([[7, "a"], [3, "b"]])) throw new Error("an @entries map must encode as a pairs array");

const back = decodeAccount(JSON.parse(JSON.stringify(wire)));
if (back.accountID !== big) throw new Error("i64 lost precision: " + back.accountID);
if (back.secret.length !== 4 || back.secret[3] !== 254) throw new Error("bytes round-trip failed");
if (back.status !== "active") throw new Error("status round-trip failed");
if (back.tip !== 500n) throw new Error("optional i64 round-trip failed");

// An open enum decodes an unknown tag leniently and preserves it: a string for
// the string-backed enum, an integer for the int-backed one.
const unknown = decodeAccount({
  account_id: "1",
  secret: "AAEC/g==",
  status: "frozen",
  code: 418,
  method: { type: "card", last4: "0000" },
  counts: [],
});
if (unknown.status !== "frozen") throw new Error("unknown enum value must pass through");
if (unknown.code !== 418) throw new Error("unknown int-backed enum value must pass through");

// Error discrimination: (status, body code) maps to the declared type, rooted
// in the taxonomy, with the @retryable predicate lowered.
const declined = decodeCreateChargeError(402, JSON.stringify({ code: "payment_declined", message: "no funds" }));
if (!(declined instanceof PaymentDeclinedError)) throw new Error("(402, code) must map to the declared error type");
if (!(declined instanceof APIError && declined instanceof TonoError)) throw new Error("a declared error must be rooted in the taxonomy");
if (!declined.retryable()) throw new Error("@retryable must lower to retryable() === true");
if (declined.data.message !== "no funds") throw new Error("the declared error body must decode");

// A status alone discriminates when unambiguous; without @retryable the
// predicate reports false.
const limited = decodeCreateChargeError(429, "{}");
if (!(limited instanceof RateLimitedError)) throw new Error("a bare status must discriminate when unambiguous");
if (limited.retryable()) throw new Error("a non-retryable error must report false");

// No match resolves to the concrete fallback type, never a declared one.
const wrongCode = decodeCreateChargeError(402, JSON.stringify({ code: "other" }));
if (wrongCode instanceof PaymentDeclinedError || !(wrongCode instanceof APIError)) throw new Error("an unmatched code must fall back to APIError");
const undeclared = decodeCreateChargeError(500, "not json");
if (!(undeclared instanceof APIError)) throw new Error("an undeclared response must fall back to APIError");
if (undeclared.status !== 500 || undeclared.body !== "not json") throw new Error("the fallback must keep the status and raw body");

console.log("ROUNDTRIP_OK");
"#;

#[test]
fn generated_typescript_compiles_and_round_trips() {
    let ws = workspace();
    let (Some(prettier), Some(tsc)) = (tool(&ws, "prettier"), tool(&ws, "tsc")) else {
        eprintln!("skipping: TypeScript toolchain not installed (run `npm ci` in {ws:?})");
        return;
    };

    // Generate the module and format it with the engine's formatter (prettier).
    // TypeScript splits each module into a types file and a serde file; write both
    // as `models`/`models_serde` plus the driver into the workspace.
    let formatter = Formatter::new(
        prettier.to_string_lossy(),
        vec!["--parser".into(), "typescript".into()],
    );
    let work = ws.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    for module_file in emit_module(&demo_module(), &ts_casing()) {
        let formatted = render_file_with_companion(
            &module_file.file,
            module_file.imports_companion.as_deref(),
            &TsRules,
            &formatter,
        );
        assert!(
            formatted.warning.is_none(),
            "prettier must format cleanly: {:?}",
            formatted.warning
        );
        std::fs::write(
            work.join(format!("models{}.ts", module_file.suffix)),
            &formatted.text,
        )
        .expect("write models source");
    }
    std::fs::write(work.join("driver.ts"), DRIVER).expect("write driver.ts");

    // Compile with tsc (a compile error is a generation bug), then run the driver.
    let compile = Command::new(&tsc)
        .current_dir(&ws)
        .output()
        .expect("run tsc");
    assert!(
        compile.status.success(),
        "tsc failed on generated code:\n{}\n{}",
        String::from_utf8_lossy(&compile.stdout),
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new("node")
        .arg(ws.join("dist/driver.js"))
        .output()
        .expect("run node");
    assert!(
        run.status.success(),
        "round-trip driver failed:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("ROUNDTRIP_OK"),
        "driver did not report success"
    );
}

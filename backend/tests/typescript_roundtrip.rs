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

use tono_backend::codegen::render::render_file;
use tono_backend::codegen::targets::typescript::emit::emit_module;
use tono_backend::codegen::targets::typescript::types::ts_casing;
use tono_backend::codegen::targets::typescript::TsRules;
use tono_backend::codegen::Formatter;
use tono_backend::ir::{EnumBacking, Member, Module, Prim, Shape, ShapeKind, Tref};

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/typescript")
}

fn tool(ws: &Path, name: &str) -> Option<PathBuf> {
    let bin = ws.join("node_modules/.bin").join(name);
    bin.exists().then_some(bin)
}

fn member(name: &str, target: Tref) -> Member {
    Member {
        name: name.into(),
        target,
        required: true,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn demo_module() -> Module {
    Module {
        name: "models".into(),
        shapes: vec![
            Shape {
                id: "models#Account".into(),
                kind: ShapeKind::Structure {
                    params: vec![],
                    members: vec![
                        member("account_id", Tref::Prim(Prim::I64)),
                        member("secret", Tref::Prim(Prim::Bytes)),
                        member(
                            "status",
                            Tref::Ref {
                                id: "models#Status".into(),
                                args: vec![],
                            },
                        ),
                    ],
                },
                traits: vec![],
            },
            Shape {
                id: "models#Status".into(),
                kind: ShapeKind::Enum {
                    backing: EnumBacking::String,
                    values: vec![("active".into(), None), ("closed".into(), None)],
                },
                traits: vec![],
            },
        ],
        operations: vec![],
    }
}

const DRIVER: &str = r#"
import { encodeAccount, decodeAccount, Account } from "./models";

const big = 9007199254740993n; // 2^53 + 1, not representable as a JS number

const account: Account = {
  accountID: big,
  secret: new Uint8Array([1, 2, 3, 254]),
  status: "active",
};

const wire: any = encodeAccount(account);
if (typeof wire.account_id !== "string") throw new Error("i64 must encode to a string");
if (wire.account_id !== big.toString()) throw new Error("i64 wire value wrong");

const back = decodeAccount(JSON.parse(JSON.stringify(wire)));
if (back.accountID !== big) throw new Error("i64 lost precision: " + back.accountID);
if (back.secret.length !== 4 || back.secret[3] !== 254) throw new Error("bytes round-trip failed");
if (back.status !== "active") throw new Error("status round-trip failed");

// An open enum decodes an unknown tag leniently and preserves it.
const unknown = decodeAccount({ account_id: "1", secret: "AAEC/g==", status: "frozen" });
if (unknown.status !== "frozen") throw new Error("unknown enum value must pass through");

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
    let file = emit_module(&demo_module(), &ts_casing());
    let formatter = Formatter::new(
        prettier.to_string_lossy(),
        vec!["--parser".into(), "typescript".into()],
    );
    let formatted = render_file(&file, &TsRules, &formatter);
    assert!(
        formatted.warning.is_none(),
        "prettier must format cleanly: {:?}",
        formatted.warning
    );

    // Write the generated module and the driver into the workspace.
    let work = ws.join("work");
    std::fs::create_dir_all(&work).expect("create work dir");
    std::fs::write(work.join("models.ts"), &formatted.text).expect("write models.ts");
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

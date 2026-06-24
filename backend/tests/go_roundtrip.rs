//! End-to-end check that the Go the engine emits compiles and round-trips.
//!
//! Generates a module, prepends the package clause, formats it with gofmt, writes
//! it as the `models` file of a small Go module, and runs that module's driver
//! with `go run`. The driver asserts the hard wire cases hold (i64 above 2^53 as
//! a string, bytes as base64, internally-tagged union, open-enum lenient decode,
//! canonical round-trip). Skips cleanly if the toolchain is absent.

use std::path::{Path, PathBuf};
use std::process::Command;

use tono_backend::codegen::render::render_file;
use tono_backend::codegen::targets::go::emit::{emit_module, package_clause};
use tono_backend::codegen::targets::go::types::go_casing;
use tono_backend::codegen::targets::go::GoRules;
use tono_backend::codegen::Formatter;
use tono_backend::ir::{EnumBacking, Member, Module, Prim, Shape, ShapeKind, Tref};

fn module_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("codegen-tests/go")
}

fn have(tool: &str, probe: &str) -> bool {
    Command::new(tool)
        .arg(probe)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn member(name: &str, target: Tref, required: bool) -> Member {
    Member {
        name: name.into(),
        target,
        required,
        default: None,
        constraints: vec![],
        traits: vec![],
    }
}

fn reference(id: &str) -> Tref {
    Tref::Ref {
        id: id.into(),
        args: vec![],
    }
}

fn structure(id: &str, members: Vec<Member>) -> Shape {
    Shape {
        id: id.into(),
        kind: ShapeKind::Structure {
            params: vec![],
            members,
        },
        traits: vec![],
    }
}

fn demo_module() -> Module {
    Module {
        name: "models".into(),
        shapes: vec![
            structure(
                "models#Account",
                vec![
                    member("account_id", Tref::Prim(Prim::I64), true),
                    member("secret", Tref::Prim(Prim::Bytes), true),
                    member("tip", Tref::Prim(Prim::I64), false),
                    member("status", reference("models#Status"), true),
                    member("method", reference("models#Method"), true),
                ],
            ),
            Shape {
                id: "models#Status".into(),
                kind: ShapeKind::Enum {
                    backing: EnumBacking::String,
                    values: vec![("active".into(), None), ("closed".into(), None)],
                },
                traits: vec![],
            },
            Shape {
                id: "models#Method".into(),
                kind: ShapeKind::Union {
                    params: vec![],
                    discriminator: "type".into(),
                    members: vec![
                        member("card", reference("models#CardData"), true),
                        member("bank", reference("models#BankData"), true),
                    ],
                },
                traits: vec![],
            },
            structure(
                "models#CardData",
                vec![member("last4", Tref::Prim(Prim::String), true)],
            ),
            structure(
                "models#BankData",
                vec![member("iban", Tref::Prim(Prim::String), true)],
            ),
        ],
        operations: vec![],
    }
}

#[test]
fn generated_go_compiles_and_round_trips() {
    // Skip under coverage: this test shells out to `go run`, which compiles a
    // separate module. A dedicated CI job runs it with a plain `cargo test`.
    if std::env::var_os("CARGO_LLVM_COV").is_some() {
        eprintln!("skipping under cargo-llvm-cov; run via `cargo test --test go_roundtrip`");
        return;
    }
    if !have("go", "version") || !have("gofmt", "-h") {
        eprintln!("skipping: Go toolchain (go/gofmt) not available");
        return;
    }
    let dir = module_dir();

    // Render the module (the package clause is prepended, since the rendered file
    // begins with imports), then format the whole with gofmt.
    let file = emit_module(&demo_module(), &go_casing());
    let rough = render_file(&file, &GoRules, &Formatter::new("cat", vec![])).text;
    // The harness compiles the generated file together with the driver in one
    // `package main`, so the clause names `main`, not the IR module.
    let source = format!("{}\n{}", package_clause("main"), rough);
    let formatted = Formatter::new("gofmt", vec![]).run(&source);
    assert!(
        formatted.warning.is_none(),
        "gofmt must format cleanly: {:?}",
        formatted.warning
    );

    std::fs::write(dir.join("models.go"), &formatted.text).expect("write models.go");

    let run = Command::new("go")
        .arg("run")
        .arg(".")
        .current_dir(&dir)
        .output()
        .expect("run go");
    assert!(
        run.status.success(),
        "generated Go failed to build or run:\n{}\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(
        String::from_utf8_lossy(&run.stdout).contains("ROUNDTRIP_OK"),
        "driver did not report success:\n{}",
        String::from_utf8_lossy(&run.stdout)
    );
}

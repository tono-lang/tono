//! The `tono` command line: turn IR JSON into SDK source files.
//!
//! `tono gen --target <list> --out <dir> [<ir.json>]` reads the IR (from the file
//! argument, or stdin when omitted), decodes it, generates the per-target source
//! through the engine, formats each file with that language's formatter, and
//! writes it under `<dir>/<target>/`. The generation itself lives in the testable
//! `tono_backend::codegen` library; this binary is the IO shell around it.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use tono_backend::codegen::{generate, parse_targets, Formatter, TargetKind};
use tono_backend::ir::decode_model;

const USAGE: &str = "usage: tono (gen --target <list> --out <dir> [<ir.json>] | version)";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("{msg}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("gen") => run_gen(&args[2..]),
        Some("version") | None => {
            println!("tono {}", tono_backend::version());
            Ok(())
        }
        Some(other) => Err(format!("unknown command: {other}\n{USAGE}")),
    }
}

fn run_gen(args: &[String]) -> Result<(), String> {
    let mut targets_csv: Option<String> = None;
    let mut out: Option<String> = None;
    let mut ir_path: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => targets_csv = Some(flag_value(args, &mut i, "--target")?),
            "--out" => out = Some(flag_value(args, &mut i, "--out")?),
            path => ir_path = Some(path.to_string()),
        }
        i += 1;
    }

    let targets = parse_targets(&targets_csv.ok_or("missing --target")?)?;
    let out_root = PathBuf::from(out.ok_or("missing --out")?);

    let json = match &ir_path {
        Some(path) => fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?,
        None => read_stdin()?,
    };
    let model = decode_model(&json)?;

    for file in generate(&model, &targets) {
        let formatted = formatter_for(file.target).run(&file.text).text;
        write_file(&out_root.join(&file.path), &formatted)?;
    }
    Ok(())
}

/// Consume the value that follows a flag, advancing the cursor past it.
fn flag_value(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

/// The formatter for a target. A missing binary degrades to the rough text (the
/// [`Formatter`] never fails), so generation works without the toolchain, just
/// less prettily.
fn formatter_for(target: TargetKind) -> Formatter {
    match target {
        TargetKind::Rust => Formatter::new("rustfmt", vec!["--edition".into(), "2021".into()]),
        TargetKind::Go => Formatter::new("gofmt", vec![]),
        TargetKind::TypeScript => {
            Formatter::new("prettier", vec!["--parser".into(), "typescript".into()])
        }
    }
}

fn read_stdin() -> Result<String, String> {
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

fn write_file(dest: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    fs::write(dest, text).map_err(|e| format!("{}: {e}", dest.display()))
}

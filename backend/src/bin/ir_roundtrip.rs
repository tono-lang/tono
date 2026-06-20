//! Reads an IR JSON document on stdin, decodes it through the mirror, and
//! re-encodes it, then checks the result equals the input as JSON data. Used by
//! the cross-language gate to drive frontend output through the backend; exits
//! non-zero (printing the difference) on any divergence.

use std::io::Read;

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("failed to read stdin");
    match tono_backend::ir::check_roundtrip(&input) {
        Ok(rt) if rt.equal => {}
        Ok(rt) => {
            eprintln!(
                "round-trip mismatch:\n  original:  {}\n  reencoded: {}",
                rt.original, rt.reencoded
            );
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

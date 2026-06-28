// The conformance driver: read a canonical wire JSON from stdin, decode it into
// the generated types, re-encode it, and print the result. The conformance
// harness pipes the same fixture to every language and asserts the re-encoded
// JSON is Value-equal across all of them.
#![allow(dead_code)]

#[path = "../models.rs"]
mod models;
// The generated serde file: the helper modules and the open enum's impls, which
// reference the types through `use crate::models::*`.
#[path = "../models_serde.rs"]
mod models_serde;

use models::Account;
use std::io::Read;

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("read stdin");
    let account: Account = serde_json::from_str(&input).expect("decode");
    let out = serde_json::to_string(&account).expect("encode");
    println!("{out}");
}

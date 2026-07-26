// The conformance driver: read a batch of wire JSON documents from stdin (one
// per line), decode each into the generated types, re-encode it, and print one
// document per line. The harnesses pipe the same batch to every language and
// compare the re-encoded JSON Value-wise across all of them.
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
    for line in input.lines().filter(|l| !l.trim().is_empty()) {
        let account: Account = serde_json::from_str(line).expect("decode");
        let out = serde_json::to_string(&account).expect("encode");
        println!("{out}");
    }
}

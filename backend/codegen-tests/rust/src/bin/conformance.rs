// The conformance driver: read a JSON array of wire documents from stdin,
// decode and re-encode each into the generated types, and print one line per
// document: the re-encoded JSON, or REJECT for a document the SDK refuses. The
// conformance harness pipes the same documents to every language and asserts
// the lines agree across all of them, so the three have to refuse the same
// malformed input, not just agree on the canonical one.

use sdk::models::Account;
use std::io::Read;

const REJECT: &str = "REJECT";

fn main() {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .expect("read stdin");
    let documents: Vec<serde_json::Value> =
        serde_json::from_str(&input).expect("stdin is a json array of documents");
    for document in documents {
        let line = serde_json::from_value::<Account>(document)
            .ok()
            .and_then(|account| serde_json::to_string(&account).ok())
            .unwrap_or_else(|| REJECT.to_string());
        println!("{line}");
    }
}

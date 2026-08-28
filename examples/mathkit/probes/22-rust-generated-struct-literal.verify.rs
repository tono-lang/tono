//! Runs the generated SDK against the stand-in crate for real: the memo is
//! instantiated over the generated `Reading` type from a literal the SDK
//! builds out of the constructor's own arguments, and what `recall` answers
//! is that `Reading`.
use mathkit_sdk::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(2.5, "base".to_string())
        .await
        .expect("construct");
    let got = client.recall().expect("recall");
    assert_eq!(got.value, 2.5, "recall value");
    assert_eq!(got.label, "base", "recall label");
    println!("probe 22 (rust generated struct literal): ok");
}

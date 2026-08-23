//! Runs the generated SDK against the stand-in crate for real: the memo is
//! instantiated over the generated `Reading` type, and what `recall` answers
//! is the `Reading` the constructor remembered.
use example_mathkit::mathkit::types::Reading;
use example_mathkit::mathkit::Client;

#[tokio::main]
async fn main() {
    let seed = Reading {
        value: 2.5,
        label: "base".to_string(),
    };
    let client = Client::new(seed.clone()).await.expect("construct");
    let got = client.recall().expect("recall");
    assert_eq!(got.value, seed.value, "recall value");
    assert_eq!(got.label, seed.label, "recall label");
    println!("probe 14 (rust generated type reference): ok");
}

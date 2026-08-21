//! Runs the generated SDK against the stand-in crate for real: the map
//! literal the binding passes reaches `from_table` as a `HashMap<String,
//! f64>`, held behind `Box<dyn Calculator<f64>>`, and compute() answers
//! the entry keyed "answer".
use example_mathkit::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(42.0).await.expect("construct");
    let got = client.value().await.expect("value");
    assert_eq!(got, 42.0, "value");
    println!("probe 13 (rust map literal): ok");
}

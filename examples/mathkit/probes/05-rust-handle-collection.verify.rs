//! Runs the generated SDK against the stand-in crate for real: two
//! already-built handles are collected into the variadic logical
//! parameter's own list, spread into the library's `Vec<Box<dyn
//! Calculator<f64>>>`, and the fallback answers the first success.
use example_mathkit::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(2.5, 4.0).await.expect("construct");
    let got = client.value().await.expect("value");
    assert_eq!(got, 2.5, "value");
    println!("probe 05 (rust handle collection): ok");
}

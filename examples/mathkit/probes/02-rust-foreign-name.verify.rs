//! Runs the generated SDK against the stand-in crate for real: the handle is
//! held as the concrete `ConstantCalculator<f64>` the constructor returns,
//! and its trait method is reached through the trait's own path.
use mathkit_sdk::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(2.5).await.expect("construct");
    let value = client.value().await.expect("value");
    assert_eq!(value, 2.5, "value");
    println!("probe 02 (rust foreign name): ok");
}

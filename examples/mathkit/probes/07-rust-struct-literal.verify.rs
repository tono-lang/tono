//! Runs the generated SDK against the stand-in crate for real: the options
//! struct is spelled by the library's own name and its `precision` field
//! wrapped in `Some(..)` from the bare `u8` the logical parameter carries.
use mathkit_sdk::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new("22 / 7".to_string(), 2).await.expect("construct");
    let value = client.value().await.expect("value");
    assert_eq!(value, 3.14, "value rounded to the declared precision");
    println!("probe 07 (rust struct literal): ok");
}

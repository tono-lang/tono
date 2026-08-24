//! The Rust driver of the FFI bench: constructs the generated client against
//! the real stand-in crate (no stub anywhere) and checks the values that come
//! back through both operations. The fallback answers its first calculator,
//! the constant one, so combined_value is the base; the series answers its
//! last value.
use mathkit_sdk::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(1.5, "2 * 3".to_string(), 2, vec![1.0, 2.0, 3.0])
        .await
        .expect("construct");
    let combined = client.combined_value().await.expect("combined_value");
    assert_eq!(combined, 1.5, "combined_value");
    let series = client.series_value().await.expect("series_value");
    assert_eq!(series, 3.0, "series_value");
    println!("ffi bench (rust): ok");
}

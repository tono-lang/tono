//! Runs the generated SDK against the stand-in crate for real: the client
//! holds the crate's `Client` (not its own generated `Client`) and reads
//! through it.
use mathkit_sdk::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new("calc.local".to_string())
        .await
        .expect("construct");
    let got = client.ping().expect("ping");
    assert_eq!(got, "pong from calc.local", "ping");
    println!("probe 13 (rust name collision): ok");
}

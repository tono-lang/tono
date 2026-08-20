//! Runs the generated SDK against the stand-in crate for real: the client
//! holds the value `FormulaCalculator::parse(expr)` returned, an associated
//! function reached through the type, behind `Box<dyn Calculator<f64>>`,
//! and compute() evaluates the formula.
use example_mathkit::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new("6 * 7".to_string()).await.expect("construct");
    let got = client.value().await.expect("value");
    assert_eq!(got, 42.0, "value");
    println!("probe 11 (rust static method): ok");
}

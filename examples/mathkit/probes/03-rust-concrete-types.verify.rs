//! Runs the generated SDK against the stand-in crate for real: two
//! constructors of the same logical handle return two different concrete
//! types, both held behind `Box<dyn Calculator<f64>>`, and each answers its
//! own operation. The series calculator answers its last value.
use example_mathkit::mathkit::Client;

#[tokio::main]
async fn main() {
    let client = Client::new(2.5, vec![1.0, 2.0, 3.0])
        .await
        .expect("construct");
    let constant = client.constant_value().await.expect("constant_value");
    assert_eq!(constant, 2.5, "constant_value");
    let series = client.series_value().await.expect("series_value");
    assert_eq!(series, 3.0, "series_value");
    println!("probe 03 (rust concrete types): ok");
}

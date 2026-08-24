//! The Rust driver of the payments example: constructs the generated entry
//! client, points it at a local in-process server (via the entry's declared
//! @env override, the only construction-time seam this entry exposes), and
//! drives one real op call through the generated crate end to end: request
//! assembly (header binding, endpoint ref), the transport call, and response
//! decoding (including a cross-module union field, same as the Go driver).
use payments_sdk::payments::charges::{Charge, Client};
use payments_sdk::payments::common::{Card, PaymentMethod, Status};
use payments_sdk::support::Timestamp;

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();

    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let header_end = loop {
            let n = socket.read(&mut chunk).await.expect("read headers");
            assert!(n > 0, "connection closed before headers completed");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = find_subslice(&buf, b"\r\n\r\n") {
                break pos;
            }
        };
        let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
        let content_length: usize = headers
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                lower
                    .strip_prefix("content-length:")
                    .map(|v| v.trim().parse().expect("content-length"))
            })
            .unwrap_or(0);
        let body_start = header_end + 4;
        while buf.len() < body_start + content_length {
            let n = socket.read(&mut chunk).await.expect("read body");
            assert!(n > 0, "connection closed before body completed");
            buf.extend_from_slice(&chunk[..n]);
        }
        let body =
            String::from_utf8_lossy(&buf[body_start..body_start + content_length]).to_string();

        assert!(
            headers.starts_with("POST /charges "),
            "unexpected request line: {headers}"
        );
        assert!(
            headers.to_ascii_lowercase().contains("x-api-key: test-key"),
            "declared header did not resolve from the client values: {headers}"
        );
        // i64 rides the wire as a string, not a bare JSON number.
        assert!(
            body.contains("\"amount\":\"1000\""),
            "request body missing the bound member: {body}"
        );

        // i64/u64 ride the wire as strings, not bare JSON numbers.
        let response_body = "{\"id\":\"c1\",\"amount\":\"1000\",\"fee\":\"0\",\"receipt\":\"aGk=\",\"currency\":\"usd\",\
             \"tags\":[],\"metadata\":{},\"created\":\"2024-01-01T00:00:00Z\",\"status\":\"active\",\
             \"method\":{\"kind\":\"card\",\"last4\":\"4242\"}}";
        // create_charge declares @http(code: 201): the server must answer
        // exactly that status for the client to decode the body as a success.
        let response = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body,
        );
        socket
            .write_all(response.as_bytes())
            .await
            .expect("write response");
        socket.flush().await.expect("flush");
    });

    // The entry's base URL resolves only from @env (no @with override is
    // declared for it), so the env var is the construction-time seam this
    // verify driver has to point at the local server.
    std::env::set_var("PAYMENTS_ENDPOINT", format!("http://127.0.0.1:{port}"));

    let client = Client::builder("test-key".to_string())
        .build()
        .expect("build client");

    // `fee` is @deprecated on the schema itself (folded into `amount`); a
    // consumer still has to set it (the field is not optional), same as any
    // other caller migrating off it.
    #[allow(deprecated)]
    let input = Charge {
        id: "ignored-by-the-server".to_string(),
        amount: 1000,
        fee: 0,
        receipt: b"hi".to_vec(),
        currency: "usd".to_string(),
        note: None,
        tags: vec![],
        metadata: Default::default(),
        created: Timestamp("2024-01-01T00:00:00Z".to_string()),
        status: Status::Active,
        method: PaymentMethod::Card(Card {
            last4: "4242".to_string(),
        }),
    };

    let charge = client.create_charge(input).await.expect("create_charge");
    assert_eq!(charge.id, "c1");
    assert_eq!(charge.amount, 1000);
    assert_eq!(charge.currency, "usd");
    assert_eq!(charge.receipt, b"hi");
    match charge.method {
        PaymentMethod::Card(card) => assert_eq!(card.last4, "4242"),
        other => panic!("union payload did not survive the round trip: {other:?}"),
    }

    server.await.expect("server task");
    println!("rust runtime verify: ok");
}

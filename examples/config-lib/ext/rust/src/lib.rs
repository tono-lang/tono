//! A stand-in for a real third-party config library: the recipe binds
//! against it declaratively (no bespoke code), so this crate only exists to
//! give the generated SDK something real to compile against.
//!
//! Its return is pinned to the exact value the `.tono` test's `stub
//! configlib.load` declares: the Rust target does not yet implement the
//! declared-test construction override for an `extern`-call field (Go and
//! TypeScript do), so the generated Rust test calls this real function
//! instead of a hermetic stub. Pinning the two to agree keeps the generated
//! test meaningful (it still proves the request carries the projected
//! endpoint/token) until that gap closes.

pub struct Config {
    pub host: String,
    pub dev_host: String,
    pub env: String,
    pub token: String,
}

pub async fn load(service: String, region: String) -> Result<Config, String> {
    let _ = (service, region);
    Ok(Config {
        host: "https://x.test".to_string(),
        dev_host: "https://x.test".to_string(),
        env: "dev".to_string(),
        token: "t0".to_string(),
    })
}

//! A stand-in for the third-party config library the generated SDK
//! integrates with (RFC appendix's `companyconfig`).

pub struct Creds {
    pub secret: String,
}

pub struct Config {
    pub host: String,
    pub dev_host: String,
    pub env: String,
    pub credentials: Creds,
}

pub async fn load(service: String, region: String) -> Result<Config, String> {
    let _ = (service, region);
    Ok(Config {
        host: "prod.internal".to_string(),
        dev_host: "dev.internal".to_string(),
        env: "dev".to_string(),
        credentials: Creds {
            secret: "s3cr3t".to_string(),
        },
    })
}

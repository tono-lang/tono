//! A stand-in for a third-party settings-provider library: a provider
//! constructed once, whose methods resolve the endpoints several operations
//! of the generated SDK read.

pub struct Config {
    pub read_url: String,
    pub write_url: String,
}

// Deliberately not `Clone`: the generated SDK reads the provider's methods
// through a borrow of its slot, and must never need to copy it.
pub struct Provider {
    name: String,
}

pub async fn new_provider(name: String) -> Result<Provider, String> {
    Ok(Provider { name })
}

impl Provider {
    pub async fn get(&self) -> Result<Config, String> {
        Ok(Config {
            read_url: format!("https://read.{}", self.name),
            write_url: format!("https://write.{}", self.name),
        })
    }

    pub async fn get_for(&self, region: String) -> Result<Config, String> {
        Ok(Config {
            read_url: format!("https://{region}.read.{}", self.name),
            write_url: format!("https://{region}.write.{}", self.name),
        })
    }
}

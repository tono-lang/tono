//! A stand-in for the third-party message-bus library the generated SDK
//! integrates with (RFC appendix's `companybus`): a free constructor that
//! returns an opaque handle, and a method on that handle the generated
//! SDK's own op body calls into.

pub struct Ack {
    pub id: String,
    pub accepted: bool,
}

// Deliberately not `Clone`: a real handle (a connection, a pool, a
// provider) typically is not, and the generated SDK must move it rather
// than assume it can be cloned.
pub struct Publisher {
    endpoint: String,
}

pub async fn connect(endpoint: String, token: String) -> Result<Publisher, String> {
    let _ = token;
    Ok(Publisher { endpoint })
}

impl Publisher {
    pub async fn send(&self, topic: String, body: String) -> Result<Ack, String> {
        if topic == "busy" {
            return Err("busy".to_string());
        }
        Ok(Ack {
            id: format!("{}:{}:{}", self.endpoint, topic, body),
            accepted: true,
        })
    }
}

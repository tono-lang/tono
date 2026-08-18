//! A stand-in for a third-party message-bus library the generated SDK
//! integrates with: a free constructor that returns an opaque handle, a
//! second free constructor that takes that handle by value and returns
//! another, and a method on the second handle the generated SDK's own op
//! body calls into.

pub struct Ack {
    pub id: String,
    pub accepted: bool,
}

// Deliberately not `Clone`: a real handle (a connection, a pool, a
// provider) typically is not, and the generated SDK must move it rather
// than assume it can be cloned, both when it is injected and when it is
// handed on to another constructor (`attach`).
pub struct Publisher {
    endpoint: String,
}

pub async fn connect(endpoint: String, token: String) -> Result<Publisher, String> {
    let _ = token;
    Ok(Publisher { endpoint })
}

// Also not `Clone`, and owns the `Publisher` it was attached to.
pub struct Relay {
    source: Publisher,
    tag: String,
}

pub async fn attach(source: Publisher, tag: String) -> Result<Relay, String> {
    Ok(Relay { source, tag })
}

impl Relay {
    pub async fn send(&self, topic: String, body: String) -> Result<Ack, String> {
        if topic == "busy" {
            return Err("busy".to_string());
        }
        Ok(Ack {
            id: format!("{}:{}:{}:{}", self.source.endpoint, self.tag, topic, body),
            accepted: true,
        })
    }
}

//! A stand-in for a third-party message-bus library the generated SDK
//! integrates with: a free constructor that returns an opaque handle, a
//! second free constructor that takes that handle by value and returns
//! another, and a method on the second handle the generated SDK's own op
//! body calls into.

pub struct Ack {
    pub id: String,
    pub accepted: bool,
}

/// The library's own error: a variant per failure, the shape a real library
/// exposes and a generated SDK recognizes by pattern.
#[derive(Debug)]
pub enum BusError {
    Busy,
    Other(String),
}

impl std::fmt::Display for BusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BusError::Busy => write!(f, "busy"),
            BusError::Other(s) => write!(f, "{s}"),
        }
    }
}

impl std::error::Error for BusError {}

// Deliberately not `Clone`: a real handle (a connection, a pool, a
// provider) typically is not, and the generated SDK must move it rather
// than assume it can be cloned, both when it is injected and when it is
// handed on to another constructor (`attach`).
pub struct Publisher {
    endpoint: String,
}

pub async fn connect(endpoint: String, token: String) -> Result<Publisher, BusError> {
    let _ = token;
    Ok(Publisher { endpoint })
}

// Also not `Clone`, and owns the `Publisher` it was attached to.
pub struct Relay {
    source: Publisher,
    tag: String,
}

pub async fn attach(source: Publisher, tag: String) -> Result<Relay, BusError> {
    Ok(Relay { source, tag })
}

impl Relay {
    pub async fn send(&self, topic: String, body: String) -> Result<Ack, BusError> {
        if topic == "busy" {
            return Err(BusError::Busy);
        }
        Ok(Ack {
            id: format!("{}:{}:{}:{}", self.source.endpoint, self.tag, topic, body),
            accepted: true,
        })
    }
}

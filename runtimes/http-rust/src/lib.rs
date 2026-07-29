//! The hand-written HTTP transport runtime for tono-generated Rust SDKs. It
//! interprets the opaque wire descriptor: builds the request, performs the
//! call, classifies the response. It ships no auth of its own; bespoke auth
//! sets a header through [`Options::headers`] or a hook.

mod descriptor;
mod request;
mod retry;
mod runtime;

pub use descriptor::{
    Binding, DeclaredError, Part, RequestHeader, ResponseBinding, ResponsePart, RetrySpec,
    SuccessCase, TemplatePart, ValueExpr, ValueSource, WireDescriptor,
};
pub use runtime::{
    AfterResponseHook, BeforeRequestHook, BoxFuture, CanonicalRequest, CanonicalResponse,
    ExclusiveTransportError, ExecuteError, Hooks, Options, Outcome, OutcomeKind, Runtime,
    Transport,
};

#[cfg(test)]
mod parity;

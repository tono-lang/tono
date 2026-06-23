//! The TypeScript target: maps the IR to idiomatic TypeScript with correct wire
//! encoding for the hard cases (open enum, internally-tagged union, generics,
//! nullable, i64-as-string, bytes-as-base64, branded well-known types).

pub mod symbols;

//! A stand-in for a real third-party settings source library that is generic
//! over the value it resolves: the recipe binds against it declaratively (no
//! bespoke code), so this crate only exists to give the generated SDK
//! something real, generic, to compile against. It resolves no value: the
//! Rust target generates no test for a call hermetic only through handle
//! method stubs (it has no seam to swap the method through), so nothing
//! calls it.

use std::marker::PhantomData;

pub struct Source<T> {
    _value: PhantomData<T>,
}

impl<T> Source<T> {
    pub async fn get(&self) -> Result<T, String> {
        Err("the stand-in source resolves no value".to_string())
    }
}

pub async fn new_env_source<T>(service: String, region: String) -> Result<Source<T>, String> {
    let _ = (service, region);
    Ok(Source {
        _value: PhantomData,
    })
}

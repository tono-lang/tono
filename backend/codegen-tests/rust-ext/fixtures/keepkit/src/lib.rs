//! A stand-in for a third-party library whose exported type name differs
//! from its Go sibling on purpose: the same logical contract is `Store[T]`
//! there and `Vault<T>` here. Concrete type, inherent methods, so the
//! per-language instantiation name is the whole story.

pub struct Vault<T> {
    seed: T,
}

pub async fn open_vault<T>(seed: T) -> Result<Vault<T>, String> {
    Ok(Vault { seed })
}

impl<T: Clone> Vault<T> {
    pub async fn get(&self) -> Result<T, String> {
        Ok(self.seed.clone())
    }
}

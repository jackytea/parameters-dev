use std::collections::HashMap;
use std::hash::BuildHasher;

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

impl<S: BuildHasher + Send + Sync> SecretStore for HashMap<String, String, S> {
    fn get(&self, name: &str) -> Option<String> {
        HashMap::get(self, name).cloned()
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct NoSecrets;

impl SecretStore for NoSecrets {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
}

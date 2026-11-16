use std::sync::{PoisonError, RwLock};

use serde_json::Value;

pub const MASK: &str = "••••";

#[derive(Debug, Default)]
pub(crate) struct Redactor {
    secrets: RwLock<Vec<String>>,
}

impl Redactor {
    pub(crate) fn add(&self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        let mut secrets = self.secrets.write().unwrap_or_else(PoisonError::into_inner);
        if !secrets.iter().any(|known| known == secret) {
            secrets.push(secret.to_owned());
            secrets.sort_unstable_by_key(|known| std::cmp::Reverse(known.len()));
        }
    }

    #[must_use]
    pub(crate) fn text(&self, text: &str) -> String {
        let secrets = self.secrets.read().unwrap_or_else(PoisonError::into_inner);
        let mut redacted = text.to_owned();
        for secret in secrets.iter() {
            if redacted.contains(secret.as_str()) {
                redacted = redacted.replace(secret.as_str(), MASK);
            }
        }
        redacted
    }

    #[must_use]
    pub(crate) fn value(&self, value: &Value) -> Value {
        match value {
            Value::String(text) => Value::String(self.text(text)),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.value(item)).collect())
            }
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(key, item)| (self.text(key), self.value(item)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn masks_every_known_secret_including_overlapping_ones() {
        let redactor = Redactor::default();
        redactor.add("abc");
        redactor.add("abcdef");
        redactor.add("");
        assert_eq!(
            redactor.text("token=abcdef, short=abc"),
            "token=••••, short=••••"
        );
        assert_eq!(
            redactor.value(&json!({ "abc": ["xabcx", 3, { "deep": "abcdef" }] })),
            json!({ "••••": ["x••••x", 3, { "deep": "••••" }] })
        );
    }
}

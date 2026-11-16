use std::fmt::{self, Write};

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::document::{Definition, Workflow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ContentHash([u8; 32]);

impl ContentHash {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn of_value(value: &Value) -> Self {
        let mut canonical = String::new();
        write_canonical(value, &mut canonical);
        Self(Sha256::digest(canonical.as_bytes()).into())
    }

    #[expect(
        clippy::expect_used,
        reason = "workflow documents only contain string keys and finite numbers, so serialization cannot fail"
    )]
    fn of_serializable(value: &impl Serialize) -> Self {
        Self::of_value(&serde_json::to_value(value).expect("document serializes to JSON"))
    }
}

impl fmt::Display for ContentHash {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl Definition {
    #[must_use]
    pub fn content_hash(&self) -> ContentHash {
        let mut without_layout = self.clone();
        without_layout.layout.clear();
        ContentHash::of_serializable(&without_layout)
    }
}

impl Workflow {
    #[must_use]
    pub fn content_hash(&self) -> ContentHash {
        let mut without_layout = self.clone();
        for graph in without_layout.graphs_mut() {
            graph.layout.clear();
        }
        ContentHash::of_serializable(&without_layout)
    }
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Object(object) => {
            let mut entries: Vec<(&String, &Value)> = object.iter().collect();
            entries.sort_unstable_by(|left, right| left.0.cmp(right.0));
            out.push('{');
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out);
                out.push(':');
                write_canonical(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::String(text) => write_string(text, out),
        Value::Number(number) => {
            let _ = write!(out, "{number}");
        }
        Value::Bool(boolean) => out.push_str(if *boolean { "true" } else { "false" }),
        Value::Null => out.push_str("null"),
    }
}

fn write_string(text: &str, out: &mut String) {
    out.push_str(&Value::String(text.to_owned()).to_string());
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::document::{Position, Step};

    #[test]
    fn ignores_key_order() {
        assert_eq!(
            ContentHash::of_value(&json!({"a": 1, "b": [true, null, "x"]})),
            ContentHash::of_value(
                &serde_json::from_str(r#"{"b": [true, null, "x"], "a": 1}"#).unwrap()
            )
        );
        assert_ne!(
            ContentHash::of_value(&json!([1, 2])),
            ContentHash::of_value(&json!([2, 1]))
        );
    }

    #[test]
    fn ignores_layout_but_not_content() {
        let mut definition = Definition::default();
        definition.steps.push(Step::new("upload", "http.request"));
        let original = definition.content_hash();
        definition.layout.insert("upload", Position::new(280, 0));
        assert_eq!(definition.content_hash(), original);
        definition.note = Some("Uploads the site".into());
        assert_ne!(definition.content_hash(), original);
    }

    #[test]
    fn renders_as_lowercase_hex() {
        let hash = ContentHash::of_value(&json!(null));
        assert_eq!(hash.to_string().len(), 64);
        assert!(
            hash.to_string()
                .bytes()
                .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        );
    }
}

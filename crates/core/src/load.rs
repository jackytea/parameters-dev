use serde::Deserialize;

use crate::document::Workflow;

pub const CURRENT_VERSION: u32 = 1;
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LoadError {
    #[error("the document is {size} bytes; the limit is {MAX_DOCUMENT_BYTES} bytes")]
    TooLarge { size: usize },
    #[error("line {line}, column {column}: {message}")]
    Malformed {
        line: usize,
        column: usize,
        message: String,
    },
    #[error("the document has no `parameters` version field")]
    MissingVersion,
    #[error(
        "document version {found} is not supported; this version of Parameters reads version {CURRENT_VERSION}"
    )]
    UnsupportedVersion { found: u64 },
}

impl From<serde_json::Error> for LoadError {
    fn from(error: serde_json::Error) -> Self {
        let message = error.to_string();
        let message = message
            .rsplit_once(" at line ")
            .map_or(message.as_str(), |(head, _)| head)
            .to_owned();
        Self::Malformed {
            line: error.line(),
            column: error.column(),
            message,
        }
    }
}

#[derive(Deserialize)]
struct VersionHeader {
    parameters: Option<serde_json::Value>,
}

pub fn from_json(text: &str) -> Result<Workflow, LoadError> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(LoadError::TooLarge { size: text.len() });
    }
    if let Some((offset, _)) = text
        .char_indices()
        .find(|(_, character)| !character.is_whitespace())
        .filter(|(_, character)| *character != '{')
    {
        let before = &text[..offset];
        return Err(LoadError::Malformed {
            line: before.matches('\n').count() + 1,
            column: before
                .rsplit('\n')
                .next()
                .map_or(0, |line| line.chars().count())
                + 1,
            message: "a workflow document must be a JSON object".to_owned(),
        });
    }
    let header: VersionHeader = serde_json::from_str(text)?;
    match header.parameters {
        None => return Err(LoadError::MissingVersion),
        Some(version) if version.as_u64() != Some(u64::from(CURRENT_VERSION)) => {
            return Err(LoadError::UnsupportedVersion {
                found: version.as_u64().unwrap_or_default(),
            });
        }
        Some(_) => {}
    }
    Ok(serde_json::from_str(text)?)
}

#[must_use]
#[expect(
    clippy::expect_used,
    reason = "workflow documents only contain string keys and finite numbers, so serialization cannot fail"
)]
pub fn to_json(workflow: &Workflow) -> String {
    let mut text = serde_json::to_string_pretty(workflow).expect("workflow serializes to JSON");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_minimal_document() {
        let workflow = from_json(r#"{"parameters": 1, "name": "Empty"}"#).unwrap();
        assert_eq!(workflow, Workflow::new("Empty"));
        assert_eq!(
            to_json(&workflow),
            "{\n  \"parameters\": 1,\n  \"name\": \"Empty\"\n}\n"
        );
    }

    #[test]
    fn checks_the_version_before_the_contents() {
        assert_eq!(
            from_json(r#"{"name": "No version"}"#),
            Err(LoadError::MissingVersion)
        );
        assert_eq!(
            from_json(r#"{"parameters": 2, "name": "Future", "new_field": true}"#),
            Err(LoadError::UnsupportedVersion { found: 2 })
        );
    }

    #[test]
    fn rejects_unknown_fields_with_a_position() {
        let error = from_json("{\n  \"parameters\": 1,\n  \"name\": \"x\",\n  \"stpes\": []\n}")
            .unwrap_err();
        let LoadError::Malformed { line, message, .. } = error else {
            panic!("expected a malformed document, got {error:?}");
        };
        assert_eq!(line, 4);
        assert!(message.contains("unknown field `stpes`"), "{message}");
    }

    #[test]
    fn rejects_documents_that_are_not_objects() {
        assert_eq!(
            from_json("\n  [1, 2]"),
            Err(LoadError::Malformed {
                line: 2,
                column: 3,
                message: "a workflow document must be a JSON object".into(),
            })
        );
    }

    #[test]
    fn rejects_oversized_documents() {
        let text = " ".repeat(MAX_DOCUMENT_BYTES + 1);
        assert_eq!(
            from_json(&text),
            Err(LoadError::TooLarge {
                size: MAX_DOCUMENT_BYTES + 1
            })
        );
    }
}

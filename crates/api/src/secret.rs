use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

pub const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_SECRET_NAME: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretSummary {
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub fn check_secret_name(name: &str) -> Result<(), String> {
    let mut bytes = name.bytes();
    let valid = name.len() <= MAX_SECRET_NAME
        && bytes
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    if valid {
        Ok(())
    } else {
        Err(format!(
            "`{name}` is not a valid secret name: use letters, digits and underscores, starting with a letter or underscore, up to {MAX_SECRET_NAME} characters"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_names_are_identifiers() {
        for name in ["NETLIFY_TOKEN", "_x", "token2"] {
            assert!(check_secret_name(name).is_ok(), "{name}");
        }
        for name in ["", "2fa", "my-token", "spaced name", "é"] {
            assert!(check_secret_name(name).is_err(), "{name}");
        }
        assert!(check_secret_name(&"a".repeat(129)).is_err());
    }
}

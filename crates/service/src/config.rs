use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use parameters_store::ConnectOptions;

use crate::cipher::EncryptionKey;

const DEFAULT_MAX_RUNS: usize = 4;
const MAX_RUNS: usize = 64;
const DEFAULT_CONNECTIONS: u32 = 10;
const MAX_CONNECTIONS: u32 = 500;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{name}: {problem}")]
pub struct ConfigError {
    pub name: &'static str,
    pub problem: String,
}

impl ConfigError {
    fn new(name: &'static str, problem: impl Into<String>) -> Self {
        Self {
            name,
            problem: problem.into(),
        }
    }
}

#[derive(Clone)]
pub struct Config {
    pub database: ConnectOptions,
    pub database_connections: u32,
    pub encryption_key: EncryptionKey,
    pub address: SocketAddr,
    pub workspace: PathBuf,
    pub max_runs: usize,
}

impl fmt::Debug for Config {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Config")
            .field("database_connections", &self.database_connections)
            .field("address", &self.address)
            .field("workspace", &self.workspace)
            .field("max_runs", &self.max_runs)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn from_env(default_address: SocketAddr) -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var(name).ok(), default_address)
    }

    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        default_address: SocketAddr,
    ) -> Result<Self, ConfigError> {
        let value = |name: &str| lookup(name).filter(|value| !value.trim().is_empty());
        let database = match value("DATABASE_URL") {
            Some(url) => url.parse::<ConnectOptions>().map_err(|_| {
                ConfigError::new("DATABASE_URL", "is not a valid PostgreSQL connection URL")
            })?,
            None => ConnectOptions::new(),
        };
        let encryption_key = value("PARAMETERS_ENCRYPTION_KEY")
            .ok_or_else(|| {
                ConfigError::new(
                    "PARAMETERS_ENCRYPTION_KEY",
                    "is not set; generate one with `openssl rand -hex 32` and keep it safe",
                )
            })
            .and_then(|key| {
                EncryptionKey::from_hex(&key).map_err(|error| {
                    ConfigError::new("PARAMETERS_ENCRYPTION_KEY", error.to_string())
                })
            })?;
        let address = match value("PARAMETERS_ADDRESS") {
            Some(address) => address.trim().parse().map_err(|_| {
                ConfigError::new(
                    "PARAMETERS_ADDRESS",
                    "must be an address such as 127.0.0.1:8080",
                )
            })?,
            None => default_address,
        };
        let allow_non_loopback = match value("PARAMETERS_ALLOW_NON_LOOPBACK").as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            Some(_) => {
                return Err(ConfigError::new(
                    "PARAMETERS_ALLOW_NON_LOOPBACK",
                    "must be true or false",
                ));
            }
        };
        if !address.ip().is_loopback() && !allow_non_loopback {
            return Err(ConfigError::new(
                "PARAMETERS_ADDRESS",
                format!(
                    "{address} is not a loopback address; local mode has no accounts and can run commands, so it only listens on this machine"
                ),
            ));
        }
        let workspace = match value("PARAMETERS_WORKSPACE") {
            Some(directory) => PathBuf::from(directory),
            None => std::env::current_dir().map_err(|error| {
                ConfigError::new(
                    "PARAMETERS_WORKSPACE",
                    format!("is not set and the current directory cannot be read: {error}"),
                )
            })?,
        };
        Ok(Self {
            database,
            database_connections: number(
                &value,
                "PARAMETERS_DATABASE_CONNECTIONS",
                DEFAULT_CONNECTIONS,
                MAX_CONNECTIONS,
            )?,
            encryption_key,
            address,
            workspace,
            max_runs: number(&value, "PARAMETERS_MAX_RUNS", DEFAULT_MAX_RUNS, MAX_RUNS)?,
        })
    }
}

fn number<T>(
    value: &impl Fn(&str) -> Option<String>,
    name: &'static str,
    default: T,
    max: T,
) -> Result<T, ConfigError>
where
    T: std::str::FromStr + PartialOrd + From<u8> + std::fmt::Display + Copy,
{
    let Some(text) = value(name) else {
        return Ok(default);
    };
    text.trim()
        .parse()
        .ok()
        .filter(|number| *number >= T::from(1) && *number <= max)
        .ok_or_else(|| ConfigError::new(name, format!("must be a whole number from 1 to {max}")))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const KEY: &str = "0000000000000000000000000000000000000000000000000000000000000001";

    fn config(pairs: &[(&str, &str)]) -> Result<Config, ConfigError> {
        let values: HashMap<String, String> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        Config::from_lookup(
            |name| values.get(name).cloned(),
            "127.0.0.1:8080".parse().unwrap(),
        )
    }

    #[test]
    fn needs_a_valid_encryption_key() {
        assert_eq!(config(&[]).unwrap_err().name, "PARAMETERS_ENCRYPTION_KEY");
        assert_eq!(
            config(&[("PARAMETERS_ENCRYPTION_KEY", "short")])
                .unwrap_err()
                .name,
            "PARAMETERS_ENCRYPTION_KEY"
        );
        let parsed = config(&[
            ("PARAMETERS_ENCRYPTION_KEY", KEY),
            ("DATABASE_URL", "postgres://user:hunter2@localhost/db"),
        ])
        .unwrap();
        assert_eq!(parsed.max_runs, DEFAULT_MAX_RUNS);
        let printed = format!("{parsed:?}");
        assert!(
            !printed.contains(KEY) && !printed.contains("hunter2"),
            "{printed}"
        );
    }

    #[test]
    fn listens_on_loopback_unless_told_otherwise() {
        let open = [
            ("PARAMETERS_ENCRYPTION_KEY", KEY),
            ("PARAMETERS_ADDRESS", "0.0.0.0:8080"),
        ];
        assert_eq!(config(&open).unwrap_err().name, "PARAMETERS_ADDRESS");
        let allowed = config(&[open[0], open[1], ("PARAMETERS_ALLOW_NON_LOOPBACK", "true")]);
        assert!(allowed.is_ok());
        assert!(
            config(&[
                ("PARAMETERS_ENCRYPTION_KEY", KEY),
                ("PARAMETERS_ADDRESS", "[::1]:9000")
            ])
            .is_ok()
        );
    }

    #[test]
    fn rejects_bad_numbers_and_urls_without_echoing_them() {
        let error = config(&[
            ("PARAMETERS_ENCRYPTION_KEY", KEY),
            ("DATABASE_URL", "postgres://user:hunter2@host:port/db"),
        ])
        .unwrap_err();
        assert_eq!(error.name, "DATABASE_URL");
        assert!(!error.to_string().contains("hunter2"));
        assert_eq!(
            config(&[
                ("PARAMETERS_ENCRYPTION_KEY", KEY),
                ("PARAMETERS_MAX_RUNS", "0")
            ])
            .unwrap_err()
            .name,
            "PARAMETERS_MAX_RUNS"
        );
    }
}

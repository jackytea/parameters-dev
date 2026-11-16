use std::fmt;
use std::net::SocketAddr;
use std::path::PathBuf;

use parameters_store::ConnectOptions;

use crate::cipher::EncryptionKey;
use crate::oauth::{Provider, ProviderKind};

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
pub struct ManagedConfig {
    pub public_url: String,
    pub smtp_url: Option<String>,
    pub mail_from: Option<String>,
    pub providers: Vec<Provider>,
    pub trust_proxy: bool,
}

#[derive(Clone)]
pub enum ModeConfig {
    Local,
    Managed(Box<ManagedConfig>),
}

#[derive(Clone)]
pub struct Config {
    pub mode: ModeConfig,
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
            .field("managed", &matches!(self.mode, ModeConfig::Managed(_)))
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
        let allow_non_loopback = flag(&value, "PARAMETERS_ALLOW_NON_LOOPBACK")?;
        let mode = match value("PARAMETERS_MODE").as_deref() {
            None | Some("local") => ModeConfig::Local,
            Some("managed") => ModeConfig::Managed(Box::new(managed(&value)?)),
            Some(_) => {
                return Err(ConfigError::new(
                    "PARAMETERS_MODE",
                    "must be local or managed",
                ));
            }
        };
        if matches!(mode, ModeConfig::Local) && !address.ip().is_loopback() && !allow_non_loopback {
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
            mode,
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

fn flag(value: &impl Fn(&str) -> Option<String>, name: &'static str) -> Result<bool, ConfigError> {
    match value(name).as_deref() {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        Some(_) => Err(ConfigError::new(name, "must be true or false")),
    }
}

fn public_url(text: &str) -> Result<String, ConfigError> {
    let invalid = |problem: &str| ConfigError::new("PARAMETERS_PUBLIC_URL", problem);
    let url = reqwest::Url::parse(text.trim())
        .map_err(|_| invalid("must be a URL such as https://parameters.dev"))?;
    let local = url
        .host_str()
        .is_some_and(|host| matches!(host, "localhost" | "127.0.0.1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err(invalid("must use https"));
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid(
            "must be just the origin, such as https://parameters.dev",
        ));
    }
    Ok(url.origin().ascii_serialization())
}

fn managed(value: &impl Fn(&str) -> Option<String>) -> Result<ManagedConfig, ConfigError> {
    let public_url = value("PARAMETERS_PUBLIC_URL")
        .ok_or_else(|| ConfigError::new("PARAMETERS_PUBLIC_URL", "is required in managed mode"))
        .and_then(|url| public_url(&url))?;
    let smtp_url = value("PARAMETERS_SMTP_URL");
    let mail_from = value("PARAMETERS_MAIL_FROM");
    if smtp_url.is_some() && mail_from.is_none() {
        return Err(ConfigError::new(
            "PARAMETERS_MAIL_FROM",
            "is required when PARAMETERS_SMTP_URL is set",
        ));
    }
    let mut providers = Vec::new();
    for kind in ProviderKind::ALL {
        let upper = kind.name().to_ascii_uppercase();
        let id = value(&format!("PARAMETERS_{upper}_CLIENT_ID"));
        let secret = value(&format!("PARAMETERS_{upper}_CLIENT_SECRET"));
        match (id, secret) {
            (Some(id), Some(secret)) => providers.push(Provider::new(kind, id, secret)),
            (None, None) => {}
            _ => {
                return Err(ConfigError::new(
                    "PARAMETERS_PUBLIC_URL",
                    format!(
                        "set both PARAMETERS_{upper}_CLIENT_ID and PARAMETERS_{upper}_CLIENT_SECRET, or neither"
                    ),
                ));
            }
        }
    }
    if smtp_url.is_none() && providers.is_empty() {
        return Err(ConfigError::new(
            "PARAMETERS_SMTP_URL",
            "managed mode needs a way to sign in: set up email (PARAMETERS_SMTP_URL) or at least one of Google, GitHub and Discord",
        ));
    }
    Ok(ManagedConfig {
        public_url,
        smtp_url,
        mail_from,
        providers,
        trust_proxy: flag(value, "PARAMETERS_TRUST_PROXY")?,
    })
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
    fn managed_mode_needs_a_public_https_url_and_a_way_to_sign_in() {
        let base = [
            ("PARAMETERS_ENCRYPTION_KEY", KEY),
            ("PARAMETERS_MODE", "managed"),
            ("PARAMETERS_ADDRESS", "0.0.0.0:8080"),
        ];
        assert_eq!(config(&base).unwrap_err().name, "PARAMETERS_PUBLIC_URL");
        let with = |extra: &[(&'static str, &'static str)]| {
            let mut pairs = base.to_vec();
            pairs.extend_from_slice(extra);
            config(&pairs)
        };
        assert_eq!(
            with(&[("PARAMETERS_PUBLIC_URL", "http://parameters.dev")])
                .unwrap_err()
                .name,
            "PARAMETERS_PUBLIC_URL"
        );
        assert_eq!(
            with(&[("PARAMETERS_PUBLIC_URL", "https://parameters.dev")])
                .unwrap_err()
                .name,
            "PARAMETERS_SMTP_URL"
        );
        let parsed = with(&[
            ("PARAMETERS_PUBLIC_URL", "https://parameters.dev/"),
            ("PARAMETERS_GITHUB_CLIENT_ID", "id"),
            ("PARAMETERS_GITHUB_CLIENT_SECRET", "hunter2"),
        ])
        .unwrap();
        let ModeConfig::Managed(managed) = &parsed.mode else {
            panic!("managed mode");
        };
        assert_eq!(managed.public_url, "https://parameters.dev");
        assert_eq!(managed.providers.len(), 1);
        assert!(!format!("{parsed:?}").contains("hunter2"));
        assert!(
            with(&[
                ("PARAMETERS_PUBLIC_URL", "https://parameters.dev"),
                ("PARAMETERS_GOOGLE_CLIENT_ID", "id"),
            ])
            .is_err()
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

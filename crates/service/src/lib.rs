mod accounts;
mod cipher;
mod config;
mod hub;
mod library;
mod mail;
mod nodes;
mod oauth;
mod page;
mod plans;
mod policy;
mod review;
mod runs;
mod secrets;
mod source;
mod triggers;
mod worker;
mod workflows;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parameters_api::{ApiError, Scope, Scopes, Via};
use parameters_engine::{Engine, EngineError, Registry};
use parameters_store::{Store, StoreError};
use uuid::Uuid;

pub use accounts::{OAuthStart, TOKEN_PREFIX};
pub use cipher::{EncryptionKey, KeyError};
pub use config::{Config, ConfigError, ManagedConfig, ModeConfig};
pub use mail::{Mail, MailFuture, Mailer, MemoryMailer, SmtpMailer};
pub use oauth::{Identity, Provider, ProviderKind};
pub use policy::Capabilities;
pub use runs::RunWatch;
pub use triggers::{HOOK_PREFIX, MAX_PAYLOAD_BYTES, PAYLOAD_INPUT};
pub use worker::Background;
pub use workflows::WorkflowWatch;

use crate::cipher::Cipher;
use crate::hub::Hub;
use crate::plans::PlanCache;
use crate::policy::RateLimiter;

pub const LOCAL_USER: Uuid = Uuid::from_u128(1);
const LOCAL_USER_NAME: &str = "Local";

pub type Result<T, E = ApiError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user: Uuid,
    pub via: Via,
    pub scopes: Option<Scopes>,
}

impl Actor {
    #[must_use]
    pub const fn local(via: Via) -> Self {
        Self {
            user: LOCAL_USER,
            via,
            scopes: None,
        }
    }

    pub fn require(&self, scope: Scope) -> Result<()> {
        if self.scopes.is_none_or(|scopes| scopes.contains(scope)) {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "this access token does not have the `{}` permission",
                scope.name()
            )))
        }
    }
}

#[derive(Clone)]
pub struct Managed {
    pub public_url: String,
    pub mailer: Option<Arc<dyn Mailer>>,
    pub providers: Vec<Provider>,
    pub trust_proxy: bool,
}

impl std::fmt::Debug for Managed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Managed")
            .field("public_url", &self.public_url)
            .field("email", &self.mailer.is_some())
            .field("providers", &self.providers)
            .field("trust_proxy", &self.trust_proxy)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub workspace: PathBuf,
    pub max_runs: usize,
    pub heartbeat: Duration,
    pub stale_after: Duration,
    pub max_attempts: i32,
    pub idle_poll: Duration,
    pub retry_delay: Option<Duration>,
    pub capabilities: Capabilities,
    pub managed: Option<Managed>,
}

impl Options {
    #[must_use]
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
            max_runs: 4,
            heartbeat: Duration::from_secs(5),
            stale_after: Duration::from_secs(30),
            max_attempts: 3,
            idle_poll: Duration::from_secs(10),
            retry_delay: None,
            capabilities: Capabilities::LOCAL,
            managed: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StartError {
    #[error("cannot reach the database: {0}")]
    Store(#[from] StoreError),
    #[error(transparent)]
    Engine(#[from] EngineError),
    #[error(transparent)]
    Key(#[from] KeyError),
    #[error("{0}")]
    Setup(String),
}

#[derive(Clone)]
pub struct Service {
    inner: Arc<Inner>,
}

struct Inner {
    store: Store,
    engine: Engine,
    cipher: Cipher,
    plans: PlanCache,
    hub: Hub,
    options: Options,
    worker: Uuid,
    limiter: RateLimiter,
    http: reqwest::Client,
}

impl Service {
    pub async fn start(config: &Config) -> Result<(Self, Background), StartError> {
        let store = Store::connect(config.database.clone(), config.database_connections).await?;
        store.migrate().await?;
        let mut options = Options::new(&config.workspace);
        options.max_runs = config.max_runs;
        if let ModeConfig::Managed(managed) = &config.mode {
            let mailer = match (&managed.smtp_url, &managed.mail_from) {
                (Some(url), Some(from)) => Some(Arc::new(
                    SmtpMailer::new(url, from).map_err(StartError::Setup)?,
                ) as Arc<dyn Mailer>),
                _ => None,
            };
            options.capabilities = Capabilities::MANAGED;
            options.managed = Some(Managed {
                public_url: managed.public_url.clone(),
                mailer,
                providers: managed.providers.clone(),
                trust_proxy: managed.trust_proxy,
            });
        }
        let service = Self::new(store, &config.encryption_key, options).await?;
        let background = service.run_in_background().await?;
        Ok((service, background))
    }

    pub async fn new(
        store: Store,
        key: &EncryptionKey,
        options: Options,
    ) -> Result<Self, StartError> {
        store.ensure_user(LOCAL_USER, LOCAL_USER_NAME).await?;
        let mut engine = Engine::new(
            Registry::builtins(),
            options.workspace.clone(),
            options.capabilities.sandbox,
        )?;
        if let Some(delay) = options.retry_delay {
            engine = engine.with_retry_delay(delay);
        }
        let cipher = Cipher::new(key)?;
        let worker = cipher.random_uuid()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|error| StartError::Setup(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                engine,
                cipher,
                plans: PlanCache::default(),
                hub: Hub::default(),
                options,
                worker,
                limiter: RateLimiter::default(),
                http,
            }),
        })
    }

    fn store(&self) -> &Store {
        &self.inner.store
    }

    fn registry(&self) -> &Registry {
        self.inner.engine.registry()
    }
}

fn internal(error: impl Display) -> ApiError {
    tracing::error!(%error, "request failed");
    ApiError::internal()
}

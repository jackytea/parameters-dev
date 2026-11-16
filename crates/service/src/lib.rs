mod cipher;
mod config;
mod hub;
mod library;
mod nodes;
mod page;
mod plans;
mod review;
mod runs;
mod secrets;
mod source;
mod worker;
mod workflows;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parameters_api::{ApiError, Via};
use parameters_engine::{Engine, EngineError, Registry};
use parameters_store::{Store, StoreError};
use uuid::Uuid;

pub use cipher::{EncryptionKey, KeyError};
pub use config::{Config, ConfigError};
pub use runs::RunWatch;
pub use worker::Background;

use crate::cipher::Cipher;
use crate::hub::Hub;
use crate::plans::PlanCache;

pub const LOCAL_USER: Uuid = Uuid::from_u128(1);
const LOCAL_USER_NAME: &str = "Local";

pub type Result<T, E = ApiError> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Actor {
    pub user: Uuid,
    pub via: Via,
}

impl Actor {
    #[must_use]
    pub const fn local(via: Via) -> Self {
        Self {
            user: LOCAL_USER,
            via,
        }
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
}

impl Service {
    pub async fn start(config: &Config) -> Result<(Self, Background), StartError> {
        let store = Store::connect(config.database.clone(), config.database_connections).await?;
        store.migrate().await?;
        let mut options = Options::new(&config.workspace);
        options.max_runs = config.max_runs;
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
        let mut engine = Engine::new(Registry::builtins(), options.workspace.clone())?;
        if let Some(delay) = options.retry_delay {
            engine = engine.with_retry_delay(delay);
        }
        let cipher = Cipher::new(key)?;
        let worker = cipher.random_uuid()?;
        Ok(Self {
            inner: Arc::new(Inner {
                store,
                engine,
                cipher,
                plans: PlanCache::default(),
                hub: Hub::default(),
                options,
                worker,
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
